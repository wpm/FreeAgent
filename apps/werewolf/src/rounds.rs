#![allow(dead_code)]
use crate::rounds::sealed::Sealed;
use crate::{PlayerId, Role};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

#[derive(Debug)]
struct State<P: Phase> {
    round: NonZero<u8>,
    phase: P,
    role: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

mod sealed {
    pub trait Sealed {}
}

pub trait Phase: Sealed {
    type Next;
}
struct Night;
impl State<Night> {
    fn new(role: HashMap<PlayerId, Role>) -> Self {
        let alive = role.keys().cloned().collect();
        Self {
            round: NonZero::new(1).unwrap(),
            phase: Night,
            role,
            alive,
        }
    }
}

struct Day;

impl Sealed for Night {}

impl Phase for Night {
    type Next = Option<State<Day>>;
}

impl Sealed for Day {}

impl Phase for Day {
    type Next = Option<State<Night>>;
}

trait Run<P: Phase> {
    fn run(&mut self, state: State<P>) -> P::Next;
}
