extern crate hyper;
#[macro_use]
extern crate futures;
extern crate bytes;
extern crate http;
extern crate httparse;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate time;

extern crate tokio;
extern crate tokio_codec;
extern crate tokio_io;
extern crate tokio_fs;

use tokio::prelude::*;
use tokio::runtime::Runtime;

extern crate rand;
extern crate sha2;
extern crate ed25519_dalek;
extern crate rust_base58;

extern crate rocksdb;

#[macro_use]
extern crate clap;
use clap::{App, ArgMatches};

mod p2p;
mod rpc;
mod account;

extern crate protos;

#[derive(Debug)]
struct Conf {
  p2p: String,
  rpc: String,
  peers: Vec<String>,
}

fn main() {
  let yaml = load_yaml!("cli.yml");

  match App::from_yaml(yaml).get_matches().subcommand() {
    ("node", Some(m)) => start_node(m),
    ("account", Some(m)) => manage_account(m),
    (s, _) => println!("{} is undefined subcommand", s),
  }
}

fn start_node(matches: &ArgMatches) {
  let p2p = matches.value_of("p2p").unwrap_or("127.0.0.1:8846");
  let rpc = matches.value_of("rpc").unwrap_or("127.0.0.1:8845");
  let peers = mk_peers(matches);
  let conf = Conf {
    p2p: p2p.to_string(),
    rpc: rpc.to_string(),
    peers: peers,
  };

  println!("{:?}", conf);

  let mut rt = Runtime::new().unwrap();
  p2p::start_server(&mut rt, &conf.p2p, conf.peers);
  println!("chat server running on {}", conf.p2p);
  rpc::start_server(&mut rt, &conf.rpc);
  println!("rpc server running on {}", conf.rpc);
  rt.shutdown_on_idle().wait().unwrap();
}

fn mk_peers(matches: &ArgMatches) -> Vec<String> {
  matches
    .value_of("peers")
    .unwrap_or("")
    .split(",")
    .map(|s| s.to_string())
    .collect()
}

fn manage_account(matches: &ArgMatches) {
  if matches.is_present("create") {
    let account = account::create();
    println!("seceret key {:?}", account.secret.as_ref().unwrap().to_bytes());
    println!("public key {:?}", account.address());
  }
}
