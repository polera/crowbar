use crate::http::models::{GrpcMessage, RequestData, RequestId, ResponseData, WsMessage};

#[derive(Debug)]
pub enum ProxyToUi {
    SessionSaved(Result<std::path::PathBuf, String>, bool),
    RequestCaptured(RequestData),
    ResponseReceived(RequestId, ResponseData),
    InterceptedRequest(RequestData),
    RequestError(RequestId, String),
    RepeaterFinished(RequestId, Result<ResponseData, String>),
    WebSocketFrame(RequestId, WsMessage),
    GrpcFrame(RequestId, GrpcMessage),
    GrpcTrailers(RequestId, Vec<(String, String)>),
    RequestCaptureTruncated(RequestId),
    MacroFinished(RequestId, usize, Result<ResponseData, String>),
}
