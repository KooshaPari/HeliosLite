use std::time::{Duration, Instant};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Controller(Option<(Uuid, Instant)>);

impl Controller {
    pub fn claim(&mut self, id: Uuid) -> anyhow::Result<()> {
        self.claim_at(id, Instant::now())
    }

    fn claim_at(&mut self, id: Uuid, now: Instant) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.0
                .is_none_or(|(owner, deadline)| owner == id || deadline <= now),
            "controller_conflict"
        );
        self.0 = Some((id, now + TTL));
        Ok(())
    }

    pub fn renew(&mut self, id: Uuid) -> bool {
        self.renew_at(id, Instant::now())
    }

    fn renew_at(&mut self, id: Uuid, now: Instant) -> bool {
        if self
            .0
            .is_some_and(|(owner, deadline)| owner == id && deadline > now)
        {
            self.0 = Some((id, now + TTL));
            true
        } else {
            false
        }
    }

    pub fn require(&mut self, id: Uuid) -> anyhow::Result<()> {
        anyhow::ensure!(self.renew(id), "controller_lease_lost");
        Ok(())
    }

    pub fn release(&mut self, id: Uuid) -> anyhow::Result<()> {
        self.require(id)?;
        self.0 = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_is_explicit_exclusive_and_expires_without_renewal() {
        let mut lease = Controller::default();
        let first = Uuid::new_v4();
        let other = Uuid::new_v4();
        let now = Instant::now();
        assert!(!lease.renew_at(first, now));
        lease.claim_at(first, now).unwrap();
        assert!(lease.claim_at(other, now).is_err());
        assert!(!lease.renew_at(other, now));
        assert!(lease.renew_at(first, now + TTL / 2));
        assert!(lease.claim_at(other, now + TTL).is_err());
        assert!(!lease.renew_at(first, now + TTL * 2));
        lease.claim_at(other, now + TTL * 2).unwrap();
        assert!(!lease.renew_at(first, now + TTL * 2));
    }

    #[test]
    fn release_cannot_evict_another_controller() {
        let mut lease = Controller::default();
        let first = Uuid::new_v4();
        lease.claim(first).unwrap();
        assert!(lease.release(Uuid::new_v4()).is_err());
        assert!(lease.renew(first));
        lease.release(first).unwrap();
        assert!(!lease.renew(first));
        lease.claim(Uuid::new_v4()).unwrap();
    }
}
