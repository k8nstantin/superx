//! Claude Desktop's log records. The kernel keeps each record whole as a
//! `desktop_event`, its text as the payload's `line`: the app's lifecycle,
//! its connectors and its updates, and no conversation, which the app keeps
//! in the cloud. Every record is read, and none feeds a table.

use crate::crawler::lines::facts::Read;
use crate::crawler::lines::kernel::Body;

/// The agent, as the kernel names it.
pub const AGENT: &str = superx_kernel::adapters::claude_desktop::AGENT_NAME;

/// One record, read: a record of text feeds no table.
#[must_use]
pub fn read_kept(body: &Body) -> Read {
    match body {
        Body::Text(_) => Read::nothing(),
        Body::Json(_) | Body::NotJson | Body::Missing => Read::unknown(),
    }
}
