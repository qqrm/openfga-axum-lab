//! Pure domain rules shared by the server and the browser demonstration.
//!
//! These functions do not perform authentication, database I/O or OpenFGA
//! requests. An allowed lifecycle transition is a domain decision, not proof
//! that the corresponding external operation has completed.
//!
//! # Provisioning and deletion
//!
//! A pet becomes visible only after provisioning. Beginning deletion hides it
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
//! assert!(valid_id("fox-123"));
//! assert!(valid_id(&"x".repeat(64)));
//! assert!(!valid_id(&"x".repeat(65)));
//! assert!(!valid_id("pet:fox"));
//! assert!(!valid_id("../fox"));
//! ```
//!
//! Text limits count UTF-8 bytes, not characters. Names and species must also
//! contain non-whitespace text; an empty biography is allowed.
//!
//! ```
//! use openfga_axum_lab::domain::valid_pet;
//!
//! assert!(valid_pet("Mika", "fox", ""));
//! assert!(!valid_pet("   ", "fox", ""));
//! assert!(!valid_pet(&"я".repeat(51), "fox", ""));
//! assert!(!valid_pet("Mika", "fox", &"x".repeat(2001)));
//! ```

#![deny(missing_docs)]

pub mod domain;
