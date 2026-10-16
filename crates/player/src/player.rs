use crate::state::{Cmd, Fact, Intent, State};
use alloc::{boxed::Box, vec::Vec};

/// Everything that can happen to a player.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Do(Intent),
    Saw(Fact),
    Key(char),
}

/// A plugin watches events and may ask for things. That is the whole plugin API.
/// `out` already holds the intents asked for so far, so a plugin may also change or drop them.
pub trait Plugin {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>);
}

/// Any closure with the right shape is a plugin.
impl<F: FnMut(&State, &Event, &mut Vec<Intent>)> Plugin for F {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        self(state, event, out)
    }
}

#[derive(Default)]
pub struct Player {
    pub state: State,
    plugins: Vec<Box<dyn Plugin>>,
}

impl Player {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, plugin: impl Plugin + 'static) -> Self {
        self.add(plugin);
        self
    }

    pub fn add(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn dispatch(&mut self, event: Event) -> Vec<Cmd> {
        let mut intents = Vec::new();
        match &event {
            Event::Do(intent) => {
                // A new source: the old one's seekable window says nothing about it. Done here, before the
                // plugins, because a plugin may take the load over (HLS does) and the intent never reaches State.
                if let Intent::Load(_) = intent {
                    self.state.seekable = None;
                }
                intents.push(intent.clone());
            }
            Event::Saw(fact) => self.state.apply(fact),
            Event::Key(_) => {}
        }
        for plugin in &mut self.plugins {
            plugin.on(&self.state, &event, &mut intents);
        }
        let mut cmds = Vec::new();
        for intent in intents {
            self.state.resolve(intent, &mut cmds);
        }
        cmds
    }
}
