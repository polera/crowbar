use crate::http::models::{RequestData, RequestId, ResponseData};

#[derive(Debug)]
pub enum ProxyToUi {
    RequestCaptured(RequestData),
    ResponseReceived(RequestId, ResponseData),
    InterceptedRequest(RequestData),
    RequestError(RequestId, String),
    RepeaterResponse(ResponseData),
    RepeaterError(String),
}
