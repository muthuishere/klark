//! The last check before an agent prompt's Enter reaches the PTY.
//!
//! `agent.prompt` refuses a blocked agent up front, but the prompt text is
//! written first and Enter only after a submit delay. If the agent raises a
//! dialog (a tool-permission prompt) inside that delay, Enter would answer the
//! dialog: usually by approving its highlighted default. The PTY writer asks
//! this guard immediately before writing Enter and drops the Enter when the
//! agent is now blocked. It also asks before typing the text, for a prompt
//! that waited in the input queue behind another one: text typed into a
//! dialog is keystrokes too.

use std::sync::Arc;

/// Answers "is the agent blocked right now?" at Enter time.
#[derive(Clone)]
pub(crate) struct EnterGuard(Arc<dyn Fn() -> bool + Send + Sync>);

impl EnterGuard {
    pub(crate) fn new(blocked: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(blocked))
    }

    pub(crate) fn agent_blocked(&self) -> bool {
        (self.0)()
    }
}

impl std::fmt::Debug for EnterGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnterGuard")
    }
}

/// The submission was stopped before Enter because the agent became blocked.
#[derive(Debug)]
struct AgentBecameBlocked;

impl std::fmt::Display for AgentBecameBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("agent became blocked before Enter; nothing was submitted")
    }
}

impl std::error::Error for AgentBecameBlocked {}

/// The submission was stopped before its text: the agent was already blocked
/// when its turn in the input queue came. Nothing was typed.
#[derive(Debug)]
struct AgentBlockedBeforeTyping;

impl std::fmt::Display for AgentBlockedBeforeTyping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("agent was blocked before the prompt was typed; nothing was typed")
    }
}

impl std::error::Error for AgentBlockedBeforeTyping {}

pub(crate) fn agent_blocked_before_typing_error() -> std::io::Error {
    std::io::Error::other(AgentBlockedBeforeTyping)
}

pub(crate) fn is_agent_blocked_before_typing(err: &std::io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<AgentBlockedBeforeTyping>())
}

pub(crate) fn agent_became_blocked_error() -> std::io::Error {
    std::io::Error::other(AgentBecameBlocked)
}

pub(crate) fn is_agent_became_blocked(err: &std::io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<AgentBecameBlocked>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_error_is_recognised_and_other_errors_are_not() {
        assert!(is_agent_became_blocked(&agent_became_blocked_error()));
        assert!(is_agent_blocked_before_typing(
            &agent_blocked_before_typing_error()
        ));
        assert!(!is_agent_became_blocked(
            &agent_blocked_before_typing_error()
        ));
        assert!(!is_agent_blocked_before_typing(
            &agent_became_blocked_error()
        ));
        assert!(!is_agent_became_blocked(&std::io::Error::other("boom")));
        assert!(!is_agent_became_blocked(&std::io::Error::from(
            std::io::ErrorKind::TimedOut
        )));
    }
}
