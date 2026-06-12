"""Recruiter: roster assembly *before* the episode exists.

A recruiter assembles the roster -- lobby, matchmaking, open enrollment,
waiting for human players to wander in -- and then hands a fixed roster to a
freshly created :class:`freeagent.environment.Environment`. This keeps the
environment's startup protocol singular and simple: known roster, everyone
shows up in time or the episode aborts.

Deliberately a stub in v1 (per DESIGN.md and the project non-goals): rosters
are assigned top-down by the runner's configuration. When recruitment beyond
static configuration is needed, it will be implemented here.
"""
