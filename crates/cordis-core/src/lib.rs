//! Engine- and renderer-independent lifecycle state. Drivers perform work only
//! after admission, and report completion; removal never discards a live ledger.
use anyhow::{Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub type Generation = u64;
pub type EffectId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Active,
    Draining,
    RecoveryBlocked,
    Inactive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectState {
    Pending,
    Live,
    Releasing,
    Failed,
}
#[derive(Clone, Debug)]
pub struct Effect {
    pub kind: String,
    pub state: EffectState,
}
#[derive(Clone, Debug)]
pub struct Episode {
    pub plugin: String,
    pub realm: String,
    pub phase: Phase,
    pub bindings: BTreeMap<String, Generation>,
    pub provides: BTreeSet<String>,
    pub effects: BTreeMap<EffectId, Effect>,
    pub calls: usize,
}
#[derive(Default, Debug)]
pub struct Core {
    next: u64,
    pub episodes: BTreeMap<Generation, Episode>,
    published: BTreeMap<(String, String), Generation>,
    pub trace: Vec<String>,
    pub guest_calls: u64,
}
impl Core {
    pub fn record(&mut self, message: String) {
        if message.contains(":guest:") {
            self.guest_calls += 1;
        }
        if self.trace.len() >= 10_000 {
            self.trace.drain(..1000);
        }
        self.trace.push(message);
    }
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    pub fn begin(
        &mut self,
        plugin: &str,
        realm: &str,
        requires: &[String],
        provides: &[String],
    ) -> Result<Generation> {
        let mut bindings = BTreeMap::new();
        for key in requires {
            let provider = self
                .published
                .get(&(realm.into(), key.clone()))
                .copied()
                .ok_or_else(|| anyhow::anyhow!("missing dependency: {key}"))?;
            bindings.insert(key.clone(), provider);
        }
        for key in provides {
            ensure!(
                !self.episodes.values().any(|episode| episode.realm == realm
                    && episode.phase != Phase::Inactive
                    && episode.provides.contains(key)),
                "provider collision: {key}"
            );
        }
        let generation = self.id();
        self.episodes.insert(
            generation,
            Episode {
                plugin: plugin.into(),
                realm: realm.into(),
                phase: Phase::Loading,
                bindings,
                provides: provides.iter().cloned().collect(),
                effects: BTreeMap::new(),
                calls: 0,
            },
        );
        self.record(format!("{generation}:loading"));
        Ok(generation)
    }
    pub fn activate(&mut self, generation: Generation) -> Result<()> {
        let episode = self
            .episodes
            .get_mut(&generation)
            .ok_or_else(|| anyhow::anyhow!("unknown generation"))?;
        ensure!(episode.phase == Phase::Loading, "not loading");
        episode.phase = Phase::Active;
        for key in &episode.provides {
            self.published
                .insert((episode.realm.clone(), key.clone()), generation);
        }
        self.record(format!("{generation}:active"));
        Ok(())
    }
    pub fn admit(&self, generation: Generation) -> Result<()> {
        ensure!(
            self.episodes
                .get(&generation)
                .is_some_and(|episode| episode.phase == Phase::Active),
            "stale or inactive generation"
        );
        Ok(())
    }
    pub fn binding(&self, caller: Generation, service: &str) -> Result<Generation> {
        self.admit(caller)?;
        self.episodes[&caller]
            .bindings
            .get(service)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("undeclared service"))
    }
    pub fn acquire_pending(&mut self, owner: Generation, kind: &str) -> Result<EffectId> {
        ensure!(
            self.episodes
                .get(&owner)
                .is_some_and(|episode| matches!(episode.phase, Phase::Loading | Phase::Active)),
            "owner closed to acquisition"
        );
        ensure!(self.episodes[&owner].effects.len() < 128, "effect limit");
        let id = self.id();
        self.episodes.get_mut(&owner).unwrap().effects.insert(
            id,
            Effect {
                kind: kind.into(),
                state: EffectState::Pending,
            },
        );
        self.record(format!("{owner}:pending:{id}:{kind}"));
        Ok(id)
    }
    /// Returns true if cancellation won: the acquired resource must be released.
    pub fn commit_acquisition(&mut self, owner: Generation, id: EffectId) -> Result<bool> {
        let episode = self
            .episodes
            .get_mut(&owner)
            .ok_or_else(|| anyhow::anyhow!("unknown owner"))?;
        let closed = !matches!(episode.phase, Phase::Loading | Phase::Active);
        let effect = episode
            .effects
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown acquisition"))?;
        ensure!(effect.state == EffectState::Pending, "already committed");
        effect.state = if closed {
            EffectState::Releasing
        } else {
            EffectState::Live
        };
        Ok(closed)
    }
    pub fn release(&mut self, owner: Generation, id: EffectId) -> Result<()> {
        if self.episodes.get(&owner).is_some_and(|episode| {
            matches!(episode.phase, Phase::Draining | Phase::RecoveryBlocked)
        }) {
            ensure!(
                self.can_recover(owner),
                "provider cleanup is guarded by dependents and calls"
            );
        }
        let episode = self
            .episodes
            .get_mut(&owner)
            .ok_or_else(|| anyhow::anyhow!("unknown owner"))?;
        let effect = episode
            .effects
            .remove(&id)
            .ok_or_else(|| anyhow::anyhow!("wrong owner or stale handle"))?;
        self.record(format!("{owner}:released:{id}:{}", effect.kind));
        Ok(())
    }
    /// Unpublishes the whole dependent closure before any driver cleanup starts.
    pub fn retire(&mut self, root: Generation) -> Result<Vec<Generation>> {
        ensure!(self.episodes.contains_key(&root), "unknown generation");
        let mut closure = BTreeSet::from([root]);
        loop {
            let before = closure.len();
            for (&id, episode) in &self.episodes {
                if episode.phase != Phase::Inactive
                    && episode.bindings.values().any(|g| closure.contains(g))
                {
                    closure.insert(id);
                }
            }
            if closure.len() == before {
                break;
            }
        }
        self.published
            .retain(|_, provider| !closure.contains(provider));
        for id in &closure {
            let episode = self.episodes.get_mut(id).unwrap();
            if episode.phase != Phase::Inactive {
                episode.phase = Phase::Draining;
            }
            self.record(format!("{id}:draining"));
        }
        // A generation binds only to an already active generation, so creation
        // order is topological. Descending order drains consumers first.
        Ok(closure.into_iter().rev().collect())
    }
    pub fn can_recover(&self, generation: Generation) -> bool {
        let Some(episode) = self.episodes.get(&generation) else {
            return false;
        };
        matches!(episode.phase, Phase::Draining | Phase::RecoveryBlocked)
            && episode.calls == 0
            && !self.episodes.values().any(|consumer| {
                consumer.phase != Phase::Inactive
                    && consumer.bindings.values().any(|g| *g == generation)
            })
    }
    pub fn finish(&mut self, generation: Generation) -> Result<()> {
        ensure!(
            self.can_recover(generation),
            "dependent or call guard blocks recovery"
        );
        let episode = self.episodes.get_mut(&generation).unwrap();
        ensure!(episode.effects.is_empty(), "unresolved owned resources");
        episode.phase = Phase::Inactive;
        self.record(format!("{generation}:inactive"));
        Ok(())
    }
    pub fn failed_release(&mut self, owner: Generation, id: EffectId) -> Result<()> {
        let Some(episode) = self.episodes.get_mut(&owner) else {
            bail!("unknown owner")
        };
        episode
            .effects
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown effect"))?
            .state = EffectState::Failed;
        episode.phase = Phase::RecoveryBlocked;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (Core, Generation, Generation) {
        let mut c = Core::default();
        let a = c
            .begin("issues", "session", &[], &["issues".into()])
            .unwrap();
        c.activate(a).unwrap();
        let b = c
            .begin("calendar", "session", &["issues".into()], &[])
            .unwrap();
        c.activate(b).unwrap();
        (c, a, b)
    }
    #[test]
    fn dependent_lease_precedes_provider_pool_cleanup() {
        let (mut c, a, b) = pair();
        let pool = c.acquire_pending(a, "pool").unwrap();
        c.commit_acquisition(a, pool).unwrap();
        let lease = c.acquire_pending(b, "lease").unwrap();
        c.commit_acquisition(b, lease).unwrap();
        assert_eq!(c.retire(a).unwrap(), vec![b, a]);
        assert!(!c.can_recover(a));
        assert!(c.release(a, pool).is_err());
        assert!(c.finish(a).is_err());
        c.release(b, lease).unwrap();
        c.finish(b).unwrap();
        assert!(c.can_recover(a));
        c.release(a, pool).unwrap();
        c.finish(a).unwrap();
        let lease_pos = c
            .trace
            .iter()
            .position(|s| s.ends_with(":lease") && s.contains(":released:"))
            .unwrap();
        let pool_pos = c
            .trace
            .iter()
            .position(|s| s.ends_with(":pool") && s.contains(":released:"))
            .unwrap();
        assert!(lease_pos < pool_pos);
    }
    #[test]
    fn pending_acquisition_lands_into_release_after_retirement() {
        let (mut c, _, b) = pair();
        let id = c.acquire_pending(b, "pending-io").unwrap();
        c.retire(b).unwrap();
        assert!(c.commit_acquisition(b, id).unwrap());
        assert!(c.finish(b).is_err());
        c.release(b, id).unwrap();
        c.finish(b).unwrap();
    }
    #[test]
    fn committed_view_is_not_rebound_and_old_actions_fail() {
        let (mut c, a, b) = pair();
        c.retire(a).unwrap();
        assert_eq!(c.episodes[&b].bindings["issues"], a);
        assert!(c.admit(b).is_err());
        c.finish(b).unwrap();
        c.finish(a).unwrap();
        let a2 = c
            .begin("issues", "session", &[], &["issues".into()])
            .unwrap();
        c.activate(a2).unwrap();
        assert_ne!(a, a2);
        assert_eq!(c.episodes[&b].bindings["issues"], a);
    }
    #[test]
    fn failed_cleanup_retains_ownership() {
        let (mut c, _, b) = pair();
        let id = c.acquire_pending(b, "lease").unwrap();
        c.commit_acquisition(b, id).unwrap();
        c.retire(b).unwrap();
        c.failed_release(b, id).unwrap();
        assert!(c.finish(b).is_err());
        assert_eq!(c.episodes[&b].phase, Phase::RecoveryBlocked);
        c.release(b, id).unwrap();
        c.finish(b).unwrap();
    }
    #[test]
    fn realms_and_resource_owners_are_enforced() {
        let (mut c, a, b) = pair();
        assert!(
            c.begin("other", "other-session", &["issues".into()], &[])
                .is_err()
        );
        let id = c.acquire_pending(a, "pool").unwrap();
        assert!(c.release(b, id).is_err());
        assert!(c.binding(b, "undeclared").is_err());
    }
}
