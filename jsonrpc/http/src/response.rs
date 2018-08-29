//! Basic Request/Response structures used internally.

pub use hyper::{header, Method};
pub use http::status::StatusCode;
pub use http::header::{CONTENT_TYPE, HeaderValue};
use http;
use headers;

pub trait Responsible {}

impl Responsible for String {}
impl<U> Responsible for Box<U> {}

/// Simple server response structure
#[derive(Debug)]
pub struct Response<T: Responsible> {
	/// Response code
	pub code: StatusCode,
	/// Response content type
	pub content_type: headers::ContentType,
	/// Response body
	pub content: T,
}

impl<T: Responsible> Response<T> {

	/// Create a response with given body and 200 OK status code.
	pub fn ok(response: T) -> Self {
		Response {
			code: StatusCode::OK,
			content_type: headers::ContentType::json(),
			content: response,
		}
	}
}

/// Create a response with empty body and 200 OK status code.
pub fn empty() -> Response<String> {
		Response::ok(String::new())
}

/// Create a response for internal error.
pub fn internal_error() -> Response<String> {
	Response {
		code: StatusCode::FORBIDDEN,
		content_type: headers::ContentType::plaintext(),
		content: "Provided Host header is not whitelisted.\n".to_owned(),
	}
}

/// Create a response for not allowed hosts.
pub fn host_not_allowed() -> Response<String> {
	Response {
		code: StatusCode::FORBIDDEN,
		content_type: headers::ContentType::plaintext(),
		content: "Provided Host header is not whitelisted.\n".to_owned(),
	}
}

/// Create a response for unsupported content type.
pub fn unsupported_content_type() -> Response<String> {
	Response {
		code: StatusCode::UNSUPPORTED_MEDIA_TYPE,
		content_type: headers::ContentType::plaintext(),
		content: "Supplied content type is not allowed. Content-Type: application/json is required\n".to_owned(),
	}
}

/// Create a response for disallowed method used.
pub fn method_not_allowed() -> Response<String> {
	Response {
		code: StatusCode::METHOD_NOT_ALLOWED,
		content_type: headers::ContentType::plaintext(),
		content: "Used HTTP Method is not allowed. POST or OPTIONS is required\n".to_owned(),
	}
}

/// CORS invalid
pub fn invalid_cors() -> Response<String> {
	Response {
		code: StatusCode::FORBIDDEN,
		content_type: headers::ContentType::plaintext(),
		content: "Origin of the request is not whitelisted. CORS headers would not be sent and any side-effects were cancelled as well.\n".to_owned(),
	}
}

/// Create a response for bad request
pub fn bad_request<S: Into<String>>(msg: S) -> Response<String> {
	Response {
		code: StatusCode::BAD_REQUEST,
		content_type: headers::ContentType::plaintext(),
		content: msg.into()
	}
}

/// Create a response for too large (413)
pub fn too_large<S: Into<String>>(msg: S) -> Response<String> {
	Response {
		code: StatusCode::PAYLOAD_TOO_LARGE,
		content_type: headers::ContentType::plaintext(),
		content: msg.into()
	}
}

impl<U: Responsible> Into<http::Response<U>> for Response<U> {	
	fn into(self) -> http::Response<U> {
		http::response::Builder::new()	
		.status(self.code)
		.header(CONTENT_TYPE, HeaderValue::from_static(&format!("{}", self.content_type)))
		.body(self.content)
		.unwrap()
	}
}
