use crate::http::models::{RequestData, RequestId, ResponseData, WsMessage};

#[derive(Debug)]
pub enum ProxyToUi {
    RequestCaptured(RequestData),
    ResponseReceived(RequestId, ResponseData),
    InterceptedRequest(RequestData),
    RequestError(RequestId, String),
    RepeaterResponse(ResponseData),
    RepeaterError(String),
    WebSocketFrame(RequestId, WsMessage),
    MacroResponse(usize, ResponseData),
    MacroError(usize, String),
}
