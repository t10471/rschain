#![deny(warnings)]

use bytes::{BufMut, Bytes, BytesMut};
use futures::future::{self, Either};
use futures::sync::mpsc;
use tokio;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

use std::collections::HashMap;
use std::mem;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Once, ONCE_INIT};

type Sender = mpsc::UnboundedSender<Bytes>;

type Reciver = mpsc::UnboundedReceiver<Bytes>;

#[derive(Clone)]
struct Shared {
  inner: Arc<Mutex<Peers>>,
}

struct Peers {
  peers: HashMap<SocketAddr, Sender>,
}

fn all_peers() -> Shared {
  static mut SINGLETON: *const Shared = 0 as *const Shared;
  static ONCE: Once = ONCE_INIT;

  unsafe {
    ONCE.call_once(|| {
      let singleton = Shared {
        inner: Arc::new(Mutex::new(Peers {
          peers: HashMap::new(),
        })),
      };
      SINGLETON = mem::transmute(Box::new(singleton));
    });
    (*SINGLETON).clone()
  }
}

struct Peer {
  name: BytesMut,
  lines: Lines,
  reciver: Reciver,
  addr: SocketAddr,
}

#[derive(Debug)]
struct Lines {
  socket: TcpStream,
  reader: BytesMut,
  writer: BytesMut,
}

impl Peer {
  fn new(name: BytesMut, lines: Lines) -> (Peer, Sender) {
    let addr = lines.socket.peer_addr().unwrap();
    let (sender, reciver) = mpsc::unbounded();
    (
      Peer {
        name,
        lines,
        reciver,
        addr,
      },
      sender,
    )
  }
}

impl Future for Peer {
  type Item = ();
  type Error = io::Error;

  fn poll(&mut self) -> Poll<(), io::Error> {
    const LINES_PER_TICK: usize = 10;
    for i in 0..LINES_PER_TICK {
      match self.reciver.poll().unwrap() {
        Async::Ready(Some(v)) => {
          self.lines.buffer(&v);
          if i + 1 == LINES_PER_TICK {
            task::current().notify();
          }
        }
        _ => break,
      }
    }

    let _ = self.lines.poll_flush()?;

    while let Async::Ready(line) = self.lines.poll()? {
      println!("Received line ({:?}) : {:?}", self.name, line);

      if let Some(message) = line {
        let mut line = self.name.clone();
        line.extend_from_slice(b": ");
        line.extend_from_slice(&message);
        line.extend_from_slice(b"\r\n");

        let line = line.freeze();
        let all = all_peers().inner;
        for (addr, tx) in &all.lock().unwrap().peers {
          if *addr != self.addr {
            tx.unbounded_send(line.clone()).unwrap();
          }
        }
      } else {
        return Ok(Async::Ready(()));
      }
    }

    Ok(Async::NotReady)
  }
}

impl Drop for Peer {
  fn drop(&mut self) {
    let all = all_peers().inner;
    &all.lock().unwrap().peers.remove(&self.addr);
  }
}

impl Lines {
  fn new(socket: TcpStream) -> Self {
    Lines {
      socket,
      reader: BytesMut::new(),
      writer: BytesMut::new(),
    }
  }

  fn buffer(&mut self, line: &[u8]) {
    self.writer.reserve(line.len());
    self.writer.put(line);
  }

  fn poll_flush(&mut self) -> Poll<(), io::Error> {
    while !self.writer.is_empty() {
      let n = try_ready!(self.socket.poll_write(&self.writer));

      assert!(n > 0);

      let _ = self.writer.split_to(n);
    }

    Ok(Async::Ready(()))
  }

  fn fill_read_buf(&mut self) -> Poll<(), io::Error> {
    loop {
      self.reader.reserve(1024);
      let n = try_ready!(self.socket.read_buf(&mut self.reader));

      if n == 0 {
        return Ok(Async::Ready(()));
      }
    }
  }
}

impl Stream for Lines {
  type Item = BytesMut;
  type Error = io::Error;

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    let sock_closed = self.fill_read_buf()?.is_ready();

    let pos = self
      .reader
      .windows(2)
      .enumerate()
      .find(|&(_, bytes)| bytes == b"\r\n")
      .map(|(i, _)| i);

    if let Some(pos) = pos {
      let mut line = self.reader.split_to(pos + 2);

      line.split_off(pos);
      return Ok(Async::Ready(Some(line)));
    }

    if sock_closed {
      Ok(Async::Ready(None))
    } else {
      Ok(Async::NotReady)
    }
  }
}

fn process(socket: TcpStream) {
  let lines = Lines::new(socket);
  let connection = lines
    .into_future()
    .map_err(|(e, _)| e)
    .and_then(|(hello, lines)| {
      println!("fist message {:?}", hello);
      match hello {
        Some(h) => {
          if format!("{:?}", h) != "b\"hello\"" {
            println!("`{:?}` invalid connection", h);
            return Either::A(future::ok(()));
          }
        }
        None => {
          println!("invalid message {:?}", hello);
          return Either::A(future::ok(()));
        }
      };
      let remote = format!("{}", lines.socket.peer_addr().unwrap());
      println!("`{:?}` is joining the chat", remote);
      let (peer, tx) = Peer::new(BytesMut::from(remote), lines);
      let all = all_peers().inner;
      all.lock().unwrap().peers.insert(peer.addr, tx);
      Either::B(peer)
    })
    .map_err(|e| println!("connection error = {:?}", e));
  tokio::spawn(connection);
}

pub fn start_server(rt: &mut tokio::runtime::Runtime, addr: &String, peers: Vec<String>) {
  let addr = addr.parse().unwrap();
  let listener = TcpListener::bind(&addr).unwrap();
  let server = listener
    .incoming()
    .map_err(|e| println!("accept error = {:?}", e))
    .for_each(move |socket| {
      process(socket);
      Ok(())
    });
  rt.spawn(server);
  connect_peers(rt, peers);
}

fn connect_peers(rt: &mut tokio::runtime::Runtime, peers: Vec<String>) {
  #![allow(unused)]
  peers.iter().for_each(|p| match p.is_empty() {
    true => (),
    false => {
      let x = TcpStream::connect(&p.parse().unwrap())
        .and_then(move |s| {
          let lines = Lines::new(s);
          let addr = lines.socket.peer_addr().unwrap();
          let (peer, tx) = Peer::new(BytesMut::from(format!("{}", addr)), lines);
          match tx.unbounded_send(BytesMut::from(&b"hello\r\n"[..]).freeze().clone()) {
            Ok(_) => {
              println!("sent peer {}", peer.addr);
              let all = all_peers().inner;
              all.lock().unwrap().peers.insert(peer.addr, tx);
              Either::B(peer)
            }
            Err(e) => {
              println!("connection error = {:?}", e);
              Either::A(future::ok(()))
            }
          }
        })
        .map_err(|e| println!("connection error = {:?}", e));
      rt.spawn(x);
    }
  });
}
