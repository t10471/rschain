extern crate hyper;
#[macro_use]
extern crate futures;
extern crate bytes;
extern crate ctrlc;
extern crate http;
extern crate httparse;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate time;
extern crate dirs;

extern crate tokio;
extern crate tokio_codec;
extern crate tokio_fs;
extern crate tokio_io;
extern crate tokio_timer;

use tokio::prelude::*;
use tokio::runtime::Runtime;

extern crate ed25519_dalek;
extern crate rand;
extern crate rust_base58;
extern crate sha2;

extern crate rocksdb;

extern crate protobuf;

#[macro_use]
extern crate clap;
use clap::{App, ArgMatches};

mod account;
mod p2p;
mod rpc;

extern crate protos;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::thread;
use std::time::{Duration, Instant};

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
  println!("p2p server running on {}", conf.p2p);
  rpc::start_server(&mut rt, &conf.rpc);
  println!("rpc server running on {}", conf.rpc);
  let running = Arc::new(AtomicBool::new(true));
  let r = running.clone();
  ctrlc::set_handler(move || {
    r.store(false, Ordering::SeqCst);
  }).expect("error setting Ctrl-C handler");
  println!("waiting for Ctrl-C...");
  while running.load(Ordering::SeqCst) {}
  println!("start send bye{:?}", Instant::now());
  p2p::send_bye();
  thread::sleep(Duration::from_secs(3));
  println!("start shutdown bye{:?}", Instant::now());
  rt.shutdown_now().wait().unwrap();
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
    println!(
      "seceret key {:?}",
      account.secret.as_ref().unwrap().to_bytes()
    );
    println!("public key {:?}", account.address());
  }
}
