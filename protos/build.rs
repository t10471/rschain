extern crate protoc_rust;

use protoc_rust::Customize;

fn main() {
  protoc_rust::run(protoc_rust::Args {
      out_dir: "src",
      input: &["p2p_message.proto"],
      includes: &["."],
      customize: Customize {
        ..Default::default()
      },
  }).expect("protoc");
}