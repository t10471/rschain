#![deny(warnings)]

use futures::{future, Future};

use tokio;

use hyper;
use hyper::service::service_fn;
use hyper::{Body, Method, Request, Response, Server, StatusCode};

use serde_json;

fn handler(req: Request<Body>) -> Box<Future<Item = Response<Body>, Error = hyper::Error> + Send> {
  let mut ret = Response::builder();
  ret.header("Content-Type", "application/json");

  let body = match (req.method(), req.uri().path()) {
    (&Method::POST, "/rpc") => {
      #[derive(Serialize)]
      struct Message {
        message: &'static str,
      }
      ret.status(StatusCode::CREATED);
      serde_json::to_string(&Message {
        message: "Hello, World!",
      }).unwrap()
    }
    _ => {
      ret.status(StatusCode::NOT_FOUND);
      #[derive(Serialize)]
      struct Message {}
      serde_json::to_string(&Message {}).unwrap()
    }
  };
  Box::new(future::ok(ret.body(Body::from(body)).unwrap()))
}

pub fn start_server(rt: &mut tokio::runtime::Runtime, addr: &String) {
  let addr = addr.parse().unwrap();

  let server = Server::bind(&addr)
    .serve(|| service_fn(handler))
    .map_err(|e| eprintln!("server error: {}", e));
  rt.spawn(server);
}
