//! A model of three months of posts, for ADR 0017's stage A
//! ([plan](../../../docs/testing/hops-model.md)): no nodes run. It follows
//! each transfer by the rules: who fetches from whom, the path each takes,
//! and what each node writes in its own record.

pub mod calibrate;
pub mod meeting;
pub mod months;
pub mod records;
pub mod rng;
pub mod spread;
