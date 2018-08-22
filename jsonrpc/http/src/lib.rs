//! jsonrpc http server.
//!
//! ```no_run
//! extern crate jsonrpc_core;
//! extern crate jsonrpc_http_server;
//!
//! use jsonrpc_core::*;
//! use jsonrpc_http_server::*;
//!
//! fn main() {
//! 	let mut io = IoHandler::new();
//! 	io.add_method("say_hello", |_: Params| {
//! 		Ok(Value::String("hello".to_string()))
//! 	});
//!
//! 	let _server = ServerBuilder::new(io)
//!		.start_http(&"127.0.0.1:3030".parse().unwrap())
//!		.expect("Unable to start RPC server");
//!
//!	_server.wait();
//! }
//! ```

#![warn(missing_docs)]

extern crate unicase;
extern crate jsonrpc_server_utils as server_utils;
extern crate net2;
extern crate mime;
extern crate http;
extern crate headers;

pub extern crate jsonrpc_core;
pub extern crate hyper;

extern crate log;

mod handler;
mod response;
mod utils;

use std::io;
use std::sync::{mpsc, Arc};
use std::net::SocketAddr;

use jsonrpc_core as jsonrpc;
use jsonrpc::MetaIoHandler;
use jsonrpc::futures::{self, Future, Stream};

pub use server_utils::hosts::{Host, DomainsValidation};
pub use server_utils::cors::{AccessControlAllowOrigin, Origin};
pub use handler::ServerHandler;
pub use utils::{is_host_allowed, cors_header, CorsHeader};
pub use response::{Response, Responsible};

/// Action undertaken by a middleware.
pub enum RequestMiddlewareAction<T, U: Responsible> {
	/// Proceed with standard RPC handling
	Proceed {
		/// Should the request be processed even if invalid CORS headers are detected?
		/// This allows for side effects to take place.
		should_continue_on_invalid_cors: bool,
		/// The request object returned
		request: http::Request<T>,
	},
	/// Intercept the request and respond differently.
	Respond {
		/// Should standard hosts validation be performed?
		should_validate_hosts: bool,
		/// a future for server response
		response: Box<Future<Item=http::Response<U>, Error=hyper::Error> + Send>,
	}
}

impl<T, U: Responsible> From<Response<U>> for RequestMiddlewareAction<T, U> {
	fn from(o: Response<U>) -> Self {
		RequestMiddlewareAction::Respond {
			should_validate_hosts: true,
			response: Box::new(futures::future::ok(o.into())),
		}
	}
}

impl<T, U: Responsible> From<http::Response<U>> for RequestMiddlewareAction<T, U> {
	fn from(response: http::Response<U>) -> Self {
		RequestMiddlewareAction::Respond {
			should_validate_hosts: true,
			response: Box::new(futures::future::ok(response)),
		}
	}
}

impl<T, U: Responsible> From<http::Request<T>> for RequestMiddlewareAction<T, U> {
	fn from<T>(request: http::Request<T>) -> Self {
		RequestMiddlewareAction::Proceed {
			should_continue_on_invalid_cors: false,
			request,
		}
	}
}

/// Allows to intercept request and handle it differently.
pub trait RequestMiddleware<T, U: Responsible>: Send + Sync + 'static {
	/// Takes a request and decides how to proceed with it.
	fn on_request(&self, request: http::Request<T>) -> RequestMiddlewareAction<T, U>;
}

impl<T, U: Responsible> RequestMiddleware<T, U> for F
where F: Fn(http::Request<T>) -> RequestMiddlewareAction<T, U> + Sync + Send + 'static{
	fn on_request(&self, request: http::Request<T>) -> RequestMiddlewareAction<T, U> {
		(*self)(request)
	}
}

#[derive(Default)]
struct NoopRequestMiddleware;
impl<T, U: Responsible> RequestMiddleware<T, U> for NoopRequestMiddleware {
	fn on_request(&self, request: http::Request<T>) -> RequestMiddlewareAction<T, U> {
		RequestMiddlewareAction::Proceed {
			should_continue_on_invalid_cors: false,
			request,
		}
	}
}

/// Extracts metadata from the HTTP request.
pub trait MetaExtractor<M: jsonrpc::Metadata, T>: Sync + Send + 'static {
	/// Read the metadata from the request
	fn read_metadata(&self, _: &http::Request<T>) -> M;
}

impl<M, F, T> MetaExtractor<M, T> for F where
	M: jsonrpc::Metadata,
	F: Fn(&http::Request<T>) -> M + Sync + Send + 'static,
{
	fn read_metadata(&self, req: &http::Request<T>) -> M {
		(*self)(req)
	}
}

#[derive(Default)]
struct NoopExtractor;
impl<M: jsonrpc::Metadata + Default, T> MetaExtractor<M, T> for NoopExtractor {
	fn read_metadata(&self, _: &http::Request<T>) -> M {
		M::default()
	}
}
//
/// RPC Handler bundled with metadata extractor.
pub struct Rpc<T, M: jsonrpc::Metadata = (), S: jsonrpc::Middleware<M> = jsonrpc::NoopMiddleware> {
	/// RPC Handler
	pub handler: Arc<MetaIoHandler<M, S>>,
	/// Metadata extractor
	pub extractor: Arc<MetaExtractor<M, T>>,
}

impl<M: jsonrpc::Metadata, S: jsonrpc::Middleware<M>> Clone for Rpc<M, S> {
	fn clone(&self) -> Self {
		Rpc {
			handler: self.handler.clone(),
			extractor: self.extractor.clone(),
		}
	}
}

type AllowedHosts = Option<Vec<Host>>;
type CorsDomains = Option<Vec<AccessControlAllowOrigin>>;

/// REST -> RPC converter state.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RestApi {
	/// The REST -> RPC converter is enabled
	/// and requires `Content-Type: application/json` header
	/// (even though the body should be empty).
	/// This protects from submitting an RPC call
	/// from unwanted origins.
	Secure,
	/// The REST -> RPC converter is enabled
	/// and does not require any `Content-Type` headers.
	/// NOTE: This allows sending RPCs via HTTP forms
	/// from any website.
	Unsecure,
	/// The REST -> RPC converter is disabled.
	Disabled,
}

/// Convenient JSON-RPC HTTP Server builder.
pub struct ServerBuilder<T, U, M: jsonrpc::Metadata = (), S: jsonrpc::Middleware<M> = jsonrpc::NoopMiddleware> {
	handler: Arc<MetaIoHandler<M, S>>,
	meta_extractor: Arc<MetaExtractor<M, T>>,
	request_middleware: Arc<RequestMiddleware<T, U>>,
	cors_domains: CorsDomains,
	cors_max_age: Option<u32>,
	allowed_hosts: AllowedHosts,
	rest_api: RestApi,
	keep_alive: bool,
	threads: usize,
	max_request_body_size: usize,
}

const SENDER_PROOF: &'static str = "Server initialization awaits local address.";

impl<M: jsonrpc::Metadata + Default, S: jsonrpc::Middleware<M>> ServerBuilder<M, S> {
	/// Creates new `ServerBuilder` for given `IoHandler`.
	///
	/// By default:
	/// 1. Server is not sending any CORS headers.
	/// 2. Server is validating `Host` header.
	pub fn new<T>(handler: T) -> Self where
		T: Into<MetaIoHandler<M, S>>
	{
		Self::with_meta_extractor(handler, NoopExtractor)
	}
}

impl<M: jsonrpc::Metadata, S: jsonrpc::Middleware<M>> ServerBuilder<M, S> {
	/// Creates new `ServerBuilder` for given `IoHandler`.
	///
	/// By default:
	/// 1. Server is not sending any CORS headers.
	/// 2. Server is validating `Host` header.
	pub fn with_meta_extractor<T, E>(handler: T, extractor: E) -> Self where
		T: Into<MetaIoHandler<M, S>>,
		E: MetaExtractor<M, T>,
	{
		ServerBuilder {
			handler: Arc::new(handler.into()),
			meta_extractor: Arc::new(extractor),
			request_middleware: Arc::new(NoopRequestMiddleware::default()),
			cors_domains: None,
			cors_max_age: None,
			allowed_hosts: None,
			rest_api: RestApi::Disabled,
			keep_alive: true,
			threads: 1,
			max_request_body_size: 5 * 1024 * 1024,
		}
	}

	/// Enable the REST -> RPC converter.
	///
	/// Allows you to invoke RPCs by sending `POST /<method>/<param1>/<param2>` requests
	/// (with no body). Disabled by default.
	pub fn rest_api(mut self, rest_api: RestApi) -> Self {
		self.rest_api = rest_api;
		self
	}

	/// Sets Enables or disables HTTP keep-alive.
	///
	/// Default is true.
	pub fn keep_alive(mut self, val: bool) -> Self {
		self.keep_alive  = val;
		self
	}

	/// Sets number of threads of the server to run.
	///
	/// Panics when set to `0`.
	#[cfg(not(unix))]
	pub fn threads(mut self, _threads: usize) -> Self {
		warn!("Multi-threaded server is not available on Windows. Falling back to single thread.");
		self
	}

	/// Sets number of threads of the server to run.
	///
	/// Panics when set to `0`.
	#[cfg(unix)]
	pub fn threads(mut self, threads: usize) -> Self {
		self.threads = threads;
		self
	}

	/// Configures a list of allowed CORS origins.
	pub fn cors(mut self, cors_domains: DomainsValidation<AccessControlAllowOrigin>) -> Self {
		self.cors_domains = cors_domains.into();
		self
	}

	/// Configure CORS `AccessControlMaxAge` header returned.
	///
	/// Passing `Some(millis)` informs the client that the CORS preflight request is not necessary
	/// for at list `millis` ms.
	/// Disabled by default.
	pub fn cors_max_age<T: Into<Option<u32>>>(mut self, cors_max_age: T) -> Self {
		self.cors_max_age = cors_max_age.into();
		self
	}

	/// Configures request middleware
	pub fn request_middleware<T, U, X: RequestMiddleware<T, U>>(mut self, middleware: T) -> Self {
		self.request_middleware = Arc::new(middleware);
		self
	}

	/// Configures metadata extractor
	pub fn meta_extractor<T: MetaExtractor<M, T>>(mut self, extractor: T) -> Self {
		self.meta_extractor = Arc::new(extractor);
		self
	}

	/// Allow connections only with `Host` header set to binding address.
	pub fn allow_only_bind_host(mut self) -> Self {
		self.allowed_hosts = Some(Vec::new());
		self
	}

	/// Specify a list of valid `Host` headers. Binding address is allowed automatically.
	pub fn allowed_hosts(mut self, allowed_hosts: DomainsValidation<Host>) -> Self {
		self.allowed_hosts = allowed_hosts.into();
		self
	}

	/// Sets the maximum size of a request body in bytes (default is 5 MiB).
	pub fn max_request_body_size(mut self, val: usize) -> Self {
		self.max_request_body_size = val;
		self
	}
}

fn recv_address(local_addr_rx: mpsc::Receiver<io::Result<SocketAddr>>) -> io::Result<SocketAddr> {
	local_addr_rx.recv().map_err(|_| {
		io::Error::new(io::ErrorKind::Interrupted, "")
	})?
}

#[cfg(unix)]
fn configure_port(reuse: bool, tcp: &net2::TcpBuilder) -> io::Result<()> {
	use net2::unix::*;

	if reuse {
		try!(tcp.reuse_port(true));
	}

	Ok(())
}

#[cfg(not(unix))]
fn configure_port(_reuse: bool, _tcp: &net2::TcpBuilder) -> io::Result<()> {
    Ok(())
}
