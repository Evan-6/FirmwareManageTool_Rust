//! Eight fixed session slots. Each owns its sequence, lease and held state; the
//! device output is the union of all of them.

use crate::config::SESSIONS;
use crate::state::{InputState, Sources};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub op: u8,
    pub seq: u32,
}

impl Pending {
    pub fn is_none(&self) -> bool {
        self.op == 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub id: u32,
    /// Highest sequence accepted from this client.
    pub received: u32,
    /// Highest sequence whose control reply has been sent.
    pub completed: u32,
    pub lease_at: u32,
    pub state: InputState,
    pub pending: Pending,
    pub released: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SessionTable {
    entries: [Session; SESSIONS],
}

impl SessionTable {
    pub fn entries(&self) -> &[Session; SESSIONS] {
        &self.entries
    }

    pub fn get(&self, slot: usize) -> &Session {
        &self.entries[slot]
    }

    pub fn get_mut(&mut self, slot: usize) -> &mut Session {
        &mut self.entries[slot]
    }

    /// Session id 0 is never valid, so a lookup for it never matches.
    pub fn find(&self, id: u32) -> Option<usize> {
        if id == 0 {
            return None;
        }
        self.entries.iter().position(|s| s.id == id)
    }

    pub fn count(&self) -> u8 {
        self.entries.iter().filter(|s| s.id != 0).count() as u8
    }

    pub fn allocate(&mut self) -> Option<usize> {
        self.entries.iter().position(|s| s.id == 0)
    }

    pub fn states(&self) -> Sources {
        let mut states = [InputState::default(); SESSIONS];
        for (slot, state) in states.iter_mut().enumerate() {
            *state = self.entries[slot].state;
        }
        states
    }

    pub fn reset(&mut self) {
        self.entries = [Session::default(); SESSIONS];
    }

    pub fn completed(&mut self, id: u32, seq: u32) {
        if let Some(slot) = self.find(id) {
            self.entries[slot].completed = seq;
        }
    }
}
