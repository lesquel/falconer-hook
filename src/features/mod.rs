//! Business capabilities. Each feature has `domain`, `application` and
//! `infrastructure` layers; dependencies point inward.

pub mod agents;
pub mod events;
pub mod pairing;
pub mod push;
pub mod responses;
