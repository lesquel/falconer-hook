//! Business capabilities. Each feature has `domain`, `application` and
//! `infrastructure` layers; dependencies point inward.

pub mod agents;
pub mod doctor;
pub mod events;
pub mod integration;
pub mod mirror;
pub mod pairing;
pub mod push;
pub mod responses;
pub mod service;
pub mod setup;
