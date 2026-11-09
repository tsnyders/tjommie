//! The seam between the providers and real processes. `packages/daemon/src/cli.ts` is ported here in T3.4.

use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
