//! A fake daemon connection for the models' tests: it answers from a table and records what it was asked. No daemon,
//! no socket.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use client::{ClientMethod, Rpc, RpcError};
use protocol::ErrorCode;
use serde_json::Value;

type Answer = Box<dyn FnMut(Value) -> Result<Value, RpcError> + Send>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct FakeRpc {
    answers: Mutex<HashMap<&'static str, Answer>>,
    calls: Mutex<Vec<(&'static str, Value)>>,
}

impl FakeRpc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers method `M` with what `answer` returns for its params (as JSON). A later `on` for the same method replaces it.
    pub fn on<M: ClientMethod>(
        self,
        mut answer: impl FnMut(Value) -> Result<M::Result, RpcError> + Send + 'static,
    ) -> Self {
        let boxed: Answer = Box::new(move |params| {
            let result = answer(params)?;
            serde_json::to_value(result).map_err(|error| RpcError::BadResult(error.to_string()))
        });
        locked(&self.answers).insert(M::NAME, boxed);
        self
    }

    /// Every call so far, in order: the method's name and its params as JSON.
    pub fn calls(&self) -> Vec<(&'static str, Value)> {
        locked(&self.calls).clone()
    }

    fn answer(&self, name: &'static str, params: Value) -> Result<Value, RpcError> {
        locked(&self.calls).push((name, params.clone()));
        match locked(&self.answers).get_mut(name) {
            Some(answer) => answer(params),
            None => Err(RpcError::Daemon {
                code: ErrorCode::UnknownMethod,
                message: format!("FakeRpc has no answer for {name}"),
            }),
        }
    }
}

impl Rpc for FakeRpc {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
        let outcome = serde_json::to_value(params)
            .map_err(|error| RpcError::BadParams(error.to_string()))
            .and_then(|params| self.answer(M::NAME, params))
            .and_then(|value| {
                serde_json::from_value(value)
                    .map_err(|error| RpcError::BadResult(error.to_string()))
            });
        async move { outcome }
    }
}

/// A view model that keeps its daemon (`LoginsClient`) can hold a test's fake by reference.
impl Rpc for &FakeRpc {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
        (**self).call::<M>(params)
    }
}
