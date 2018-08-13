#[macro_use]
extern crate futures;
extern crate bytes;
extern crate http;
extern crate httparse;

#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate time;
extern crate tokio;
extern crate tokio_codec;
extern crate tokio_io;

extern crate hyper;
extern crate tokio_fs;

mod p2p;
mod rpc;

use tokio::prelude::*;
use tokio::runtime::Runtime;

#[macro_use]
extern crate clap;

use clap::App;

#[derive(Debug)]
struct Conf {
  p2p: String,
  rpc: String,
  peers: Vec<String>,
}

fn main() {
  let conf = parse_args();
  println!("{:?}", conf);
  let mut rt = Runtime::new().unwrap();
  p2p::start_server(&mut rt, &conf.p2p, conf.peers);
  println!("chat server running on {}", conf.p2p);
  rpc::start_server(&mut rt, &conf.rpc);
  println!("rpc server running on {}", conf.rpc);
  rt.shutdown_on_idle().wait().unwrap();
}

fn parse_args() -> Conf {
  let yaml = load_yaml!("cli.yml");
  let matches = App::from_yaml(yaml).get_matches();

  let p2p = matches.value_of("p2p").unwrap_or("127.0.0.1:8846");
  let rpc = matches.value_of("rpc").unwrap_or("127.0.0.1:8845");
  let peers = matches
    .value_of("peers")
    .unwrap_or("")
    .split(",")
    .map(|s| s.to_string())
    .collect();
  Conf {
    p2p: p2p.to_string(),
    rpc: rpc.to_string(),
    peers: peers,
  }
}
