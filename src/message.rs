//! What actors say to one another: the [`Message`] trait.

use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Debug;

/// What one episode's actors say to one another.
///
/// An episode carries one message type, and every actor in it sends and
/// receives that type. A message is cloned for every recipient, travels
/// between actors on a multi-threaded runtime, and is written to the log
/// and read back from it, which is what the bounds say. Implementing it
/// takes one empty line:
///
/// ```
/// # use free_agent::Message;
/// # use serde::{Deserialize, Serialize};
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Note(String);
/// impl Message for Note {}
/// ```
pub trait Message: Debug + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {}
