use crate::http::models::{RequestData, RequestId, ResponseData};

#[derive(Debug)]
pub enum ProxyToUi {
    RequestCaptured(RequestData),
    ResponseReceived(RequestId, ResponseData),
    InterceptedRequest(RequestData),
    RequestError(RequestId, String),
    StatusUpdate {
        active_connections: u32,
        total_processed: u64,
    },
    RepeaterResponse(ResponseData),
    RepeaterError(String),
}

#[derive(Debug)]
pub enum UiToProxy {
    Forward(RequestId),
    ForwardEdited(RequestId, RequestData),
    Drop(RequestId),
    SetIntercept(bool),
    RepeaterSend(RequestData),
    Shutdown,
}
