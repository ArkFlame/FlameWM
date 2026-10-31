use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_settings_core::ControlTransport;

#[derive(Default)]
pub(super) struct ScriptedTransport {
    pub(super) responses: VecDeque<Result<ControlResponse, ControlError>>,
    pub(super) requests: Rc<RefCell<Vec<ControlRequest>>>,
}

impl ControlTransport for ScriptedTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
        self.requests.borrow_mut().push(request);
        self.responses
            .pop_front()
            .expect("scripted response was not queued")
    }
}
