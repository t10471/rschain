#![deny(warnings)]

use bytes::{BufMut, Bytes, BytesMut};
use futures::future::{self, Either};
use futures::sync::mpsc;
use tokio;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};


/// Shorthand for the transmit half of the message channel.
type Tx = mpsc::UnboundedSender<Bytes>;

/// Shorthand for the receive half of the message channel.
type Rx = mpsc::UnboundedReceiver<Bytes>;

/// Data that is shared between all peers in the chat server.
pub struct Shared {
  peers: HashMap<SocketAddr, Tx>,
}

/// The state for each connected client.
struct Peer {
  /// Name of the peer.
  name: BytesMut,
  /// The TCP socket wrapped with the `Lines` codec, defined below.
  lines: Lines,
  /// Handle to the shared chat state.
  state: Arc<Mutex<Shared>>,
  /// Receive half of the message channel.
  rx: Rx,
  /// Client socket address.
  addr: SocketAddr,
}

/// Line based codec
#[derive(Debug)]
struct Lines {
  /// The TCP socket.
  socket: TcpStream,
  /// Buffer used when reading from the socket. Data is not returned from this
  /// buffer until an entire line has been read.
  rd: BytesMut,
  /// Buffer used to stage data before writing it to the socket.
  wr: BytesMut,
}

impl Shared {
  /// Create a new, empty, instance of `Shared`.
  fn new() -> Self {
    Shared { peers: HashMap::new() }
  }
}

impl Peer {
  /// Create a new instance of `Peer`.
  fn new(name: BytesMut, state: Arc<Mutex<Shared>>, lines: Lines) -> Peer {
    // Get the client socket address
    let addr = lines.socket.peer_addr().unwrap();
    // Create a channel for this peer
    let (tx, rx) = mpsc::unbounded();
    // Add an entry for this `Peer` in the shared state map.
    state.lock().unwrap().peers.insert(addr, tx);

    Peer {
      name,
      lines,
      state,
      rx,
      addr,
    }
  }
}

/// This is where a connected client is managed.
impl Future for Peer {
  type Item = ();
  type Error = io::Error;

  fn poll(&mut self) -> Poll<(), io::Error> {
    const LINES_PER_TICK: usize = 10;

    // Receive all messages from peers.
    for i in 0..LINES_PER_TICK {
      match self.rx.poll().unwrap() {
        Async::Ready(Some(v)) => {
          self.lines.buffer(&v);
          if i + 1 == LINES_PER_TICK {
            task::current().notify();
          }
        }
        _ => break,
      }
    }

    // Flush the write buffer to the socket
    let _ = self.lines.poll_flush()?;

    // Read new lines from the socket
    while let Async::Ready(line) = self.lines.poll()? {
      println!("Received line ({:?}) : {:?}", self.name, line);

      if let Some(message) = line {
        // Append the peer's name to the front of the line:
        let mut line = self.name.clone();
        line.extend_from_slice(b": ");
        line.extend_from_slice(&message);
        line.extend_from_slice(b"\r\n");

        let line = line.freeze();

        for (addr, tx) in &self.state.lock().unwrap().peers {
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
    self.state.lock().unwrap().peers.remove(&self.addr);
  }
}

impl Lines {
  /// Create a new `Lines` codec backed by the socket
  fn new(socket: TcpStream) -> Self {
    Lines {
      socket,
      rd: BytesMut::new(),
      wr: BytesMut::new(),
    }
  }

  /// Buffer a line.
  fn buffer(&mut self, line: &[u8]) {
    self.wr.reserve(line.len());
    self.wr.put(line);
  }

  /// Flush the write buffer to the socket
  fn poll_flush(&mut self) -> Poll<(), io::Error> {
    // As long as there is buffered data to write, try to write it.
    while !self.wr.is_empty() {
      // Try to write some bytes to the socket
      let n = try_ready!(self.socket.poll_write(&self.wr));

      // As long as the wr is not empty, a successful write should
      // never write 0 bytes.
      assert!(n > 0);

      // This discards the first `n` bytes of the buffer.
      let _ = self.wr.split_to(n);
    }

    Ok(Async::Ready(()))
  }

  /// Read data from the socket.
  fn fill_read_buf(&mut self) -> Poll<(), io::Error> {
    loop {
      // Ensure the read buffer has capacity.
      self.rd.reserve(1024);

      // Read data into the buffer.
      let n = try_ready!(self.socket.read_buf(&mut self.rd));

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
    // First, read any new data that might have been received off the socket
    let sock_closed = self.fill_read_buf()?.is_ready();

    // Now, try finding lines
    let pos = self
      .rd
      .windows(2)
      .enumerate()
      .find(|&(_, bytes)| bytes == b"\r\n")
      .map(|(i, _)| i);

    if let Some(pos) = pos {
      // Remove the line from the read buffer and set it to `line`.
      let mut line = self.rd.split_to(pos + 2);

      // Drop the trailing \r\n
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

/// Spawn a task to manage the socket.
fn process(socket: TcpStream, state: Arc<Mutex<Shared>>) {
  let lines = Lines::new(socket);
  let connection = lines
    .into_future()
    .map_err(|(e, _)| e)
    .and_then(|(hello, lines)| {
      match hello {
        Some(h) => {
          if format!("{:?}", h) != "hello" {
            println!("`{:?}` invalid connection", h);
            return Either::A(future::ok(()));
          }
        }
        None => {
          // The remote client closed the connection without sending any data.
          return Either::A(future::ok(()));
        }
      };
      let remote = format!("{}", lines.socket.peer_addr().unwrap());
      println!("`{:?}` is joining the chat", remote);
      let peer = Peer::new(BytesMut::from(remote), state, lines);
      Either::B(peer)
    })
    .map_err(|e| {
      println!("connection error = {:?}", e);
    });
  tokio::spawn(connection);
}

pub fn start_server(rt: &mut tokio::runtime::Runtime, addr: &String) ->  Arc<Mutex<Shared>>{
  let state = Arc::new(Mutex::new(Shared::new()));
  let addr = addr.parse().unwrap();
  let listener = TcpListener::bind(&addr).unwrap();
  let server = listener
    .incoming()
    .map_err(|e| println!("accept error = {:?}", e))
    .for_each(move |socket| {
      process(socket, state.clone());
      Ok(())
    });
  rt.spawn(server);
  state
}

pub fn connect_peers(state: Arc<Mutex<Shared>>, peers: Vec<String>) {
  peers.into_iter().map(|p| {
    TcpStream::connect(&p.parse().unwrap())
      .map_err(|e| println!("accept error = {:?}", e))
      .map(|s| {
        let lines = Lines::new(s);
        let addr = lines.socket.peer_addr().unwrap();
        Peer::new(BytesMut::from(format!("{}", addr)), state.clone(), lines);
        match state.lock().unwrap().peers.get(&addr) {
          Some(tx) => tx.unbounded_send(BytesMut::from("hello\r\n").freeze().clone()).unwrap(),
          None => print!("{} not exists", addr),
        }
      })
      .map_err(|e| {
        println!("connection error = {:?}", e);
      })
  });
}
