use crate::{Day, Night, PlayerId, State};

mod llm;

pub trait Rules {
    // one method per (phase, role) cell; the defaults are standard Werewolf
    fn werewolves_at_night(&mut self, state: &mut State<Night>, wolves: &[PlayerId]);
    fn seer_at_night(&mut self, state: &mut State<Night>, seer: &PlayerId);
    fn villagers_by_day(&mut self, state: &mut State<Day>);
}
