use serde::{Deserialize, Serialize};

use crate::http::models::{RequestData, ResponseData};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    Pending,
    Running,
    Complete,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceStep {
    pub request: RequestData,
    pub response: Option<ResponseData>,
    pub error: Option<String>,
    pub state: StepState,
}

impl SequenceStep {
    pub fn new(request: RequestData) -> Self {
        Self {
            request,
            response: None,
            error: None,
            state: StepState::Pending,
        }
    }
}
