//! The clicks that answer or close a `kova ask` question from a web view (the
//! Cockpit, the chat), queued on the delegate and served by the main loop
//! (`app.rs`), which owns the board.

/// What a click asks the board to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskAction {
    Answer { id: String, action: String, text: String },
    Snooze { id: String, until: u64 },
    /// Off the board without an answer (`Board::close`), with why.
    Close { id: String, reason: &'static str },
}

/// The queue between the click and the tick that owns the board.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskQueue {
    actions: Vec<AskAction>,
}

impl AskQueue {
    pub fn push(&mut self, action: AskAction) {
        self.actions.push(action);
    }

    /// Take everything queued. The caller serves it on the main loop.
    pub fn drain(&mut self) -> Vec<AskAction> {
        std::mem::take(&mut self.actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_drains_once() {
        let mut queue = AskQueue::default();
        queue.push(AskAction::Close { id: "a".into(), reason: "done" });
        assert_eq!(queue.drain(), vec![AskAction::Close { id: "a".into(), reason: "done" }]);
        assert!(queue.drain().is_empty());
    }
}
