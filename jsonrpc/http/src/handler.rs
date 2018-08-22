use Rpc;

use std::{fmt, mem, str};
use std::sync::Arc;

use mime;
use hyper::{self, service};
use http::{self, Method, Response};
use headers;
use unicase::Ascii;

use jsonrpc::{self as core, FutureResult, Metadata, Middleware, NoopMiddleware};
use jsonrpc::futures::{Future, Poll, Async, Stream, future};
use jsonrpc::serde_json;
use response::{self, Responsible, Response as RPCResponse};
use server_utils::cors;

use {utils, RequestMiddleware, RequestMiddlewareAction, CorsDomains, AllowedHosts, RestApi};

/// jsonrpc http request handler.
pub struct ServerHandler<T, U, M: Metadata = (), S: Middleware<M> = NoopMiddleware> {
	jsonrpc_handler: Rpc<T, M, S>,
	allowed_hosts: AllowedHosts,
	cors_domains: CorsDomains,
	cors_max_age: Option<u32>,
	middleware: Arc<RequestMiddleware<T, U>>,
	rest_api: RestApi,
	max_request_body_size: usize,
}

impl<T, U, M: Metadata, S: Middleware<M>> ServerHandler<T, U, M, S> {
	/// Create new request handler.
	pub fn new(
		jsonrpc_handler: Rpc<T, M, S>,
		cors_domains: CorsDomains,
		cors_max_age: Option<u32>,
		allowed_hosts: AllowedHosts,
		middleware: Arc<RequestMiddleware<T, U>>,
		rest_api: RestApi,
		max_request_body_size: usize,
	) -> Self {
		ServerHandler {
			jsonrpc_handler,
			allowed_hosts,
			cors_domains,
			cors_max_age,
			middleware,
			rest_api,
			max_request_body_size,
		}
	}
}

impl<T, U: Responsible, M: Metadata, S: Middleware<M>> service::Service for ServerHandler<T, U, M, S> {
	type Error = hyper::Error;
	type Future = Handler<M, S, T, U>;

	fn call(&self, request: http::Request<Self::ReqBody>) -> Self::Future {
		let is_host_allowed = utils::is_host_allowed(&request, &self.allowed_hosts);
		let action = self.middleware.on_request(request);

		let (should_validate_hosts, should_continue_on_invalid_cors, response) = match action {
			RequestMiddlewareAction::Proceed { should_continue_on_invalid_cors, request }=> (
				true, should_continue_on_invalid_cors, Err(request)
			),
			RequestMiddlewareAction::Respond { should_validate_hosts, response } => (
				should_validate_hosts, false, Ok(response)
			),
		};

		// Validate host
		if should_validate_hosts && !is_host_allowed {
			return Handler::Error(Some(response::host_not_allowed().into()));
		}

		// Replace response with the one returned by middleware.
		match response {
			Ok(response) => Handler::Middleware(response),
			Err(request) => {
				Handler::Rpc(RpcHandler {
					jsonrpc_handler: self.jsonrpc_handler.clone(),
					is_options: false,
					cors_header: cors::CorsHeader::NotRequired,
					rest_api: self.rest_api,
					cors_max_age: self.cors_max_age,
					max_request_body_size: self.max_request_body_size,
				})
			}
		}
	}
}

pub enum Handler<M: Metadata, S: Middleware<M>, T, U> {
	Rpc(RpcHandler<M, S, T, U>),
	Error(Option<Response<T>>),
	Middleware(Box<Future<Item=http::Response<T>, Error=hyper::Error> + Send>),
}

impl<M: Metadata, S: Middleware<M>, T, U> Future for Handler<M, S, T, U> {
	type Item = http::Response<T>;
	type Error = hyper::Error;

	fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
		match *self {
			Handler::Rpc(ref mut handler) => handler.poll(),
			Handler::Middleware(ref mut middleware) => middleware.poll(),
			Handler::Error(ref mut response) => Ok(Async::Ready(
				response.take().expect("Response always Some initialy. Returning `Ready` so will never be polled again; qed").into()
			)),
		}
	}
}

enum RpcPollState<M, F, T, U> where
	F: Future<Item = Option<core::Response>, Error = ()>,
{
	Ready(RpcHandlerState<M, F, T, U>),
	NotReady(RpcHandlerState<M, F, T, U>),
}

impl<M, F, T, U> RpcPollState<M, F, T, U> where
	F: Future<Item = Option<core::Response>, Error = ()>,
{
	fn decompose(self) -> (RpcHandlerState<M, F, T, U>, bool) {
		use self::RpcPollState::*;
		match self {
			Ready(handler) => (handler, true),
			NotReady(handler) => (handler, false),
		}
	}
}

enum RpcHandlerState<M, F, T, U> where
	F: Future<Item = Option<core::Response>, Error = ()>,
	U: Responsible
{
	ReadingHeaders {
		request: http::Request<T>,
		cors_domains: CorsDomains,
		continue_on_invalid_cors: bool,
	},
	ReadingBody {
		body: hyper::Body,
		uri: Option<hyper::Uri>,
		request: Vec<u8>,
		metadata: M,
	},
	ProcessRest {
		uri: hyper::Uri,
		metadata: M,
	},
	Writing(RPCResponse<U>),
	Waiting(FutureResult<F>),
	Done,
}

impl<M, F, T, U: fmt::Debug + Responsible> fmt::Debug for RpcHandlerState<M, F, T, U> where
	F: Future<Item = Option<core::Response>, Error = ()>,
{
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		use self::RpcHandlerState::*;

		match *self {
			ReadingHeaders {..} => write!(fmt, "ReadingHeaders"),
			ReadingBody {..} => write!(fmt, "ReadingBody"),
			ProcessRest {..} => write!(fmt, "ProcessRest"),
			Writing(ref res) => write!(fmt, "Writing({:?})", res),
			Waiting(_) => write!(fmt, "Waiting"),
			Done => write!(fmt, "Done"),
		}
	}
}

pub struct RpcHandler<M: Metadata, S: Middleware<M>, T, U> {
	jsonrpc_handler: Rpc<M, S>,
	is_options: bool,
	cors_header: cors::CorsHeader<headers::AccessControlAllowOrigin>,
	cors_max_age: Option<u32>,
	rest_api: RestApi,
	max_request_body_size: usize,
}

// Intermediate and internal error type to better distinguish
// error cases occuring during request body processing.
enum BodyError {
	Hyper(hyper::Error),
	Utf8(str::Utf8Error),
	TooLarge,
}

impl From<hyper::Error> for BodyError {
	fn from(e: hyper::Error) -> BodyError {
		BodyError::Hyper(e)
	}
}

impl<M: Metadata, S: Middleware<M>, T, U: Responsible> RpcHandler<M, S, T, U> {
	fn read_headers(
		&self,
		request: http::Request<T>,
		continue_on_invalid_cors: bool,
	) -> RpcHandlerState<M, S::Future, T, Box<U>> {
		if self.cors_header == cors::CorsHeader::Invalid && !continue_on_invalid_cors {
			return RpcHandlerState::Writing(response::invalid_cors());
		}
		// Read metadata
		let metadata = self.jsonrpc_handler.extractor.read_metadata(&request);

		// Proceed
		match *request.method() {
			// Validate the ContentType header
			// to prevent Cross-Origin XHRs with text/plain
			Method::POST if Self::is_json(request.headers().get::<header::ContentType>()) => {
				let uri = if self.rest_api != RestApi::Disabled { Some(request.uri().clone()) } else { None };
				RpcHandlerState::ReadingBody {
					metadata,
					request: Default::default(),
					uri,
					body: request.body(),
				}
			},
			Method::POST if self.rest_api == RestApi::Unsecure && request.uri().path().split('/').count() > 2 => {
				RpcHandlerState::ProcessRest {
					metadata,
					uri: request.uri().clone(),
				}
			},
			// Just return error for unsupported content type
			Method::POST => {
				RpcHandlerState::Writing(response::unsupported_content_type())
			},
			// Don't validate content type on options
			Method::OPTIONS => {
				RpcHandlerState::Writing(response::empty())
			},
			// Disallow other methods.
			_ => {
				RpcHandlerState::Writing(response::method_not_allowed())
			},
		}
	}

	fn process_rest(
		&self,
		uri: hyper::Uri,
		metadata: M,
	) -> Result<RpcPollState<M, S::Future, T, U>, hyper::Error> {
		use self::core::types::{Call, MethodCall, Version, Params, Request, Id, Value};

		// skip the initial /
		let mut it = uri.path().split('/').skip(1);

		// parse method & params
		let method = it.next().unwrap_or("");
		let mut params = Vec::new();
		for param in it {
			let v = serde_json::from_str(param)
				.or_else(|_| serde_json::from_str(&format!("\"{}\"", param)))
				.unwrap_or(Value::Null);
			params.push(v)
		}

		// Parse request
		let call = Request::Single(Call::MethodCall(MethodCall {
			jsonrpc: Some(Version::V2),
			method: method.into(),
			params: Params::Array(params),
			id: Id::Num(1),
		}));

		return Ok(RpcPollState::Ready(RpcHandlerState::Waiting(
			future::Either::B(self.jsonrpc_handler.handler.handle_rpc_request(call, metadata))
				.map(|res| res.map(|x| serde_json::to_string(&x)
					.expect("Serialization of response is infallible;qed")
				))
		)));
	}

	fn process_body(
		&self,
		mut body: hyper::Body,
		mut request: Vec<u8>,
		uri: Option<hyper::Uri>,
		metadata: M,
	) -> Result<RpcPollState<M, S::Future, T, U>, BodyError> {
		loop {
			match body.poll()? {
				Async::Ready(Some(chunk)) => {
					if request.len().checked_add(chunk.len()).map(|n| n > self.max_request_body_size).unwrap_or(true) {
						return Err(BodyError::TooLarge)
					}
					request.extend_from_slice(&*chunk)
				},
				Async::Ready(None) => {
					if let (Some(uri), true) = (uri, request.is_empty()) {
						return Ok(RpcPollState::Ready(RpcHandlerState::ProcessRest {
							uri,
							metadata,
						}));
					}

					let content = match str::from_utf8(&request) {
						Ok(content) => content,
						Err(err) => {
							// Return utf error.
							return Err(BodyError::Utf8(err));
						},
					};

					// Content is ready
					return Ok(RpcPollState::Ready(RpcHandlerState::Waiting(
						self.jsonrpc_handler.handler.handle_request(content, metadata)
					)));
				},
				Async::NotReady => {
					return Ok(RpcPollState::NotReady(RpcHandlerState::ReadingBody {
						body,
						request,
						metadata,
						uri,
					}));
				},
			}
		}
	}

	fn set_response_headers(
		headers: &mut headers::Headers,
		is_options: bool,
		cors_header: Option<headers::AccessControlAllowOrigin>,
		cors_max_age: Option<u32>,
	) {
		if is_options {
			headers.set(headers::AccessControlAllowMethods(vec![
				Method::OPTIONS,
				Method::POST,
			]));
			headers.set(headers::Accept(vec![
				headers::qitem(mime::APPLICATION_JSON)
			]));
		}

		if let Some(cors_domain) = cors_header {
			headers.set(headers::AccessControlAllowMethods(vec![
				Method::OPTIONS,
				Method::POST
			]));
			headers.set(headers::AccessControlAllowHeaders(vec![
				Ascii::new("origin".to_owned()),
				Ascii::new("content-type".to_owned()),
				Ascii::new("accept".to_owned()),
			]));
			if let Some(cors_max_age) = cors_max_age {
				headers.set(headers::AccessControlMaxAge(cors_max_age));
			}
			headers.set(cors_domain);
			headers.set(headers::Vary::Items(vec![
				Ascii::new("origin".to_owned())
			]));
		}
	}

	fn is_json(content_type: Option<&headers::ContentType>) -> bool {
		const APPLICATION_JSON_UTF_8: &str = "application/json; charset=utf-8";

		match content_type {
			Some(&headers::ContentType(ref mime))
				if *mime == mime::APPLICATION_JSON || *mime == APPLICATION_JSON_UTF_8 => true,
			_ => false
		}
	}
}
