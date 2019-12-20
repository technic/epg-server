use iron::prelude::*;
use iron::status;
use std::error::Error;

pub fn bad_request<E: 'static + Error + Send>(error: E) -> IronError {
    let m = (status::BadRequest, error.description().to_string());
    IronError::new(error, m)
}
