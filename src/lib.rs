//! Catalog authorization service and pure rules shared with the browser demonstration.
//!
//! The functions in [`domain`] do not perform authentication, database I/O or OpenFGA
//! requests. An allowed lifecycle transition is a domain decision, not proof
//! that the corresponding external operation has completed.
//!
//! # Provisioning and deletion
//!
//! A table becomes visible only after provisioning. Beginning deletion hides it
//! before its authorization tuples are removed.
//!
//! ```
//! use openfga_axum_lab::domain::{Event, Lifecycle, is_visible, transition};
//!
//! assert!(!is_visible(Lifecycle::Pending));
//! let active = transition(Lifecycle::Pending, Event::Provisioned).unwrap();
//! assert!(is_visible(active));
//! let deleting = transition(active, Event::BeginDeletion).unwrap();
//! assert!(!is_visible(deleting));
//! let deleted = transition(deleting, Event::TuplesRemoved).unwrap();
//! assert_eq!(deleted, Lifecycle::Deleted);
//! assert!(!is_visible(deleted));
//! ```
//!
//! # Invalid transitions are rejected
//!
//! Repeated delivery is handled by the application, not by silently accepting
//! an invalid domain transition. Deleted objects cannot be resurrected.
//!
//! ```
//! use openfga_axum_lab::domain::{Event, Lifecycle, transition};
//!
//! assert_eq!(transition(Lifecycle::Active, Event::Provisioned), None);
//! assert_eq!(transition(Lifecycle::Pending, Event::TuplesRemoved), None);
//! assert_eq!(transition(Lifecycle::Deleted, Event::Provisioned), None);
//! assert_eq!(Lifecycle::from_storage("unknown"), None);
//! ```
//!
//! # Identifiers and text limits
//!
//! Identifiers contain only ASCII letters, digits, hyphens and underscores.
//! They do not include the OpenFGA type prefix.
//!
//! ```
//! use openfga_axum_lab::domain::valid_id;
//!
//! assert!(valid_id("requests-123"));
//! assert!(valid_id(&"x".repeat(64)));
//! assert!(!valid_id(&"x".repeat(65)));
//! assert!(!valid_id("lakekeeper_table:requests"));
//! assert!(!valid_id("../review"));
//! ```
//!
//! Text limits count UTF-8 bytes, not characters. Names must contain
//! non-whitespace text; an empty description is allowed.
//!
//! ```
//! use openfga_axum_lab::domain::valid_table;
//!
//! assert!(valid_table("Request metrics", ""));
//! assert!(!valid_table("   ", ""));
//! assert!(!valid_table(&"я".repeat(51), ""));
//! assert!(!valid_table("Request metrics", &"x".repeat(2001)));
//! ```

pub mod application;
mod authentication;
pub mod authorization_migration;
mod cached_authorizer;
mod configuration;
#[deny(missing_docs)]
pub mod domain;
mod http;
pub mod infrastructure;
mod metrics;
pub mod model_binding;
mod operations;
mod server;
mod use_cases;

pub use server::run;
