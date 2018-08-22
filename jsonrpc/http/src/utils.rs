use http::{self, header};
use http::header::{HOST, ORIGIN};
use headers;

use server_utils::{cors, hosts};
pub use server_utils::cors::CorsHeader;

/// Extracts string value of a single header in request.
fn read_header<'a, T, U>(req: &'a http::Request<T>, header: U) -> Option<&'a str> 
where U: header::AsHeaderName {
	match req.headers().get(header) {
		Some(v) if v.len() == 1 => {
			v.to_str().ok()
		},
		_ => None
	}
}

/// Returns `true` if Host header in request matches a list of allowed hosts.
pub fn is_host_allowed<T>(
	request: &http::Request<T>,
	allowed_hosts: &Option<Vec<hosts::Host>>,
) -> bool {
	hosts::is_host_valid(read_header(request, HOST), allowed_hosts)
}

/// Returns a CORS header that should be returned with that request.
pub fn cors_header<T>(
	request: &http::Request<T>,
	cors_domains: &Option<Vec<cors::AccessControlAllowOrigin>>
) -> CorsHeader<headers::AccessControlAllowOrigin> {
	cors::get_cors_header(read_header(request, ORIGIN), read_header(request, HOST), cors_domains).map(|origin| {
		use self::cors::AccessControlAllowOrigin::*;
		match origin {
			Value(val) => headers::AccessControlAllowOrigin::Value((*val).to_owned()),
			Null => headers::AccessControlAllowOrigin::Null,
			Any => headers::AccessControlAllowOrigin::Any,
		}
	})
}
