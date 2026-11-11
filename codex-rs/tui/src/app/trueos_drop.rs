//! Drain the receiving Shell3 window's UI4 mailbox on the app task, never via paste.
//! A changed/unavailable conversation discards queued drops rather than retargeting them.

pub(super) struct DropInbox<S> {
    scope: Option<S>,
}

impl<S: Eq> DropInbox<S> {
    pub(super) fn new() -> Self {
        Self { scope: None }
    }

    pub(super) fn poll(
        &mut self,
        scope: Option<S>,
        mut take: impl FnMut() -> Option<Result<Vec<String>, ()>>,
        mut attach: impl FnMut(Vec<String>) -> bool,
    ) -> bool {
        let accepts = scope.is_some() && self.scope == scope;
        self.scope = scope;
        let mut rejected = false;
        // UI4 bounds its mailbox to 32 drops. Do not monopolize the app loop.
        for _ in 0..32 {
            let Some(drop) = take() else { break };
            match drop {
                Ok(paths) if accepts => rejected |= !attach(paths),
                _ => rejected = true,
            }
        }
        rejected
    }
}

#[cfg(target_os = "trueos")]
pub(super) fn take() -> Option<Result<Vec<String>, ()>> {
    let drop = trueos_v::vdrag::take_drop(0).ok()??;
    Some(if drop.kind == trueos_v::vdrag::PATH_LIST_V1 {
        crate::file_references::decode(&drop.bytes).ok_or(())
    } else {
        Err(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stay_in_the_receiving_conversation_and_are_consumed_once() {
        let mut inbox = DropInbox { scope: None };
        let mut delivered = Vec::new();
        assert!(!inbox.poll(Some((1, "thread")), || None, |_| panic!("empty")));
        let mut queue = [Ok(vec!["4:/foo.rs".into()])].into_iter();
        assert!(!inbox.poll(
            Some((1, "thread")),
            || queue.next(),
            |paths| {
                delivered.push(paths);
                true
            }
        ));
        assert_eq!(delivered, [vec!["4:/foo.rs".to_owned()]]);
        assert!(!inbox.poll(
            Some((1, "thread")),
            || queue.next(),
            |_| panic!("already taken")
        ));
        // Identical thread names on different backends must not inherit a drop.
        for scope in [Some((2, "thread")), Some((2, "other")), None] {
            let mut queue = [Ok(vec!["/late".into()])].into_iter();
            assert!(inbox.poll(scope, || queue.next(), |_| panic!("retargeted")));
            assert!(queue.next().is_none());
        }
    }

    #[test]
    fn invalid_or_blocked_drops_are_rejected_without_stalling_the_mailbox() {
        let mut inbox = DropInbox { scope: Some(1) };
        let mut queue = [Err(()), Ok(vec!["/blocked".into()])].into_iter();
        let mut attempts = 0;
        assert!(inbox.poll(
            Some(1),
            || queue.next(),
            |_| {
                attempts += 1;
                false
            }
        ));
        assert_eq!(attempts, 1);
        assert!(queue.next().is_none());
    }
}
