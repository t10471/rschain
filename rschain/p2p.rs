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

use protobuf::*;
use protos::p2p_message as p2p_m;

type Sender = mpsc::UnboundedSender<Bytes>;

type Reciver = mpsc::UnboundedReceiver<Bytes>;

const STREAM_PER_TICK: usize = 10;

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
  name: String,
  stream: P2PStream,
  reciver: Reciver,
  addr: SocketAddr,
}

#[derive(Debug)]
struct P2PStream {
  socket: TcpStream,
  reader: BytesMut,
  writer: BytesMut,
}

impl Peer {
  fn new(name: String, stream: P2PStream) -> (Peer, Sender) {
    let addr = stream.socket.peer_addr().unwrap();
    let (sender, reciver) = mpsc::unbounded();
    (
      Peer {
        name,
        stream,
        reciver,
        addr,
      },
      sender,
    )
  }

  fn write_recived(&mut self) {
    for i in 0..STREAM_PER_TICK {
      match self.reciver.poll().unwrap() {
        Async::Ready(Some(v)) => {
          self.stream.buffer(&v);
          if i + 1 == STREAM_PER_TICK {
            task::current().notify();
          }
        }
        _ => break,
      }
    }
  }
}

impl Future for Peer {
  type Item = ();
  type Error = io::Error;

  fn poll(&mut self) -> Poll<(), io::Error> {
    self.write_recived();
    self.stream.poll_flush()?;

    while let Async::Ready(stream) = self.stream.poll()? {
      if let Some(message) = stream {
        broadcast(&self.name, message.freeze(), Some(&self.addr));
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

impl P2PStream {
  fn new(socket: TcpStream) -> Self {
    P2PStream {
      socket,
      reader: BytesMut::new(),
      writer: BytesMut::new(),
    }
  }

  fn buffer(&mut self, message: &[u8]) {
    self.writer.reserve(message.len());
    self.writer.put(message);
  }

  fn poll_flush(&mut self) -> Poll<(), io::Error> {
    while !self.writer.is_empty() {
      let n = try_ready!(self.socket.poll_write(&self.writer));
      assert!(n > 0);
      self.writer.split_to(n);
    }
    Ok(Async::Ready(()))
  }

  fn fill_read_buf(&mut self) -> Poll<(), io::Error> {
    loop {
      self.reader.reserve(1024);
      if try_ready!(self.socket.read_buf(&mut self.reader)) == 0 {
        return Ok(Async::Ready(()));
      }
    }
  }
}

impl Stream for P2PStream {
  type Item = BytesMut;
  type Error = io::Error;

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    match (self.fill_read_buf()?.is_ready(), self.reader.len()) {
      (_, n) if n > 0 => Ok(Async::Ready(Some(self.reader.split_to(n)))),
      (true, _) => Ok(Async::Ready(None)),
      _ => Ok(Async::NotReady),
    }
  }
}

fn broadcast(name: &String, message: Bytes, from: Option<&SocketAddr>) {
  println!("broadcast from = {:?}, message = {:?}", name, message);
  let all = all_peers().inner;
  for (addr, tx) in &all.lock().unwrap().peers {
    match from {
      Some(f)  => {
        if f != addr {
          tx.unbounded_send(message.clone()).unwrap()
        }
      },
      
      None => tx.unbounded_send(message.clone()).unwrap()
    }    
  }
}

fn is_valid_handshake_message(message: Option<BytesMut>) -> bool {
  if let Some(msg) = message {
    match parse_from_bytes::<p2p_m::Message>(&msg)
      .unwrap()
      .get_field_type()
    {
      p2p_m::Message_MessageType::Handshake => true,
      t => {
        println!("invalid Message_MessageType `{:?}`", t);
        false
      }
    }
  } else {
    println!("recieved empty message");
    false
  }
}

fn process(socket: TcpStream) {
  let connection = P2PStream::new(socket)
    .into_future()
    .map_err(|(e, _)| e)
    .and_then(|(message, stream)| {
      println!("fist message {:?}", message);
      if !is_valid_handshake_message(message) {
        return Either::A(future::ok(()));
      }
      let remote = format!("{}", stream.socket.peer_addr().unwrap());
      println!("`{:?}` is joining p2p", remote);
      let (peer, tx) = Peer::new(remote, stream);
      let all = all_peers().inner;
      all.lock().unwrap().peers.insert(peer.addr, tx);
      Either::B(peer)
    })
    .map_err(|e| println!("connection error = {:?}", e));
  tokio::spawn(connection);
}

pub fn start_server(rt: &mut tokio::runtime::Runtime, addr: &String, peers: Vec<String>) {
  let server = TcpListener::bind(&addr.parse().unwrap())
    .unwrap()
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
  peers.iter().for_each(|p| {
    if !p.is_empty() {
      connect(rt, p)
    }
  });
}

fn connect(rt: &mut tokio::runtime::Runtime, peer: &String) {
  let x = TcpStream::connect(&peer.parse().unwrap())
    .and_then(move |s| {
      let stream = P2PStream::new(s);
      let addr = stream.socket.peer_addr().unwrap();
      let (peer, tx) = Peer::new(format!("{}", addr), stream);
      let mut msg = p2p_m::Message::new();
      msg.set_field_type(p2p_m::Message_MessageType::Handshake);
      msg.set_payload(Bytes::from(&b"handshake"[..]));
      let x = Bytes::from(msg.write_to_bytes().unwrap());
      println!("handshake: sent bytes {:?}", x);
      match tx.unbounded_send(x) {
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

pub fn send_bye() {
  let mut msg = p2p_m::Message::new();
  msg.set_field_type(p2p_m::Message_MessageType::Bye);
  msg.set_payload(Bytes::from(&b"byebye"[..]));
  let x = Bytes::from(msg.write_to_bytes().unwrap());
  println!("bye: sent bytes {:?}", x);
  broadcast(&"my".to_string(), x, None);
}