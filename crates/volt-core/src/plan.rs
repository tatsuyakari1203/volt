//! Plans: operations with previews, risk and evidence labels.
//!
//! Building a plan only reads the system, so a plan is the dry run: show it to
//! the user, and apply it only after they confirm.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PlanError;
use crate::label::{Evidence, Risk};
use crate::operation::Operation;
use crate::ports::Ports;

/// What a caller asks to be planned: an operation plus its declared labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequest {
    /// The change.
    pub operation: Operation,
    /// How dangerous the change is.
    pub risk: Risk,
    /// How well the benefit is established.
    pub evidence: Evidence,
}

impl OperationRequest {
    /// Bundles an operation with its labels.
    #[must_use]
    pub fn new(operation: Operation, risk: Risk, evidence: Evidence) -> Self {
        Self {
            operation,
            risk,
            evidence,
        }
    }
}

/// Human-readable "current -> target" for one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    /// What is changed, such as `HKCU\Software\X\Y` or `service Spooler start type`.
    pub subject: String,
    /// The value right now, as text.
    pub current: String,
    /// The value after applying, as text.
    pub target: String,
}

impl fmt::Display for Preview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} -> {}", self.subject, self.current, self.target)
    }
}

/// One operation in a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    /// The change.
    pub operation: Operation,
    /// What it will do, computed when the plan was built.
    pub preview: Preview,
    /// How dangerous the change is.
    pub risk: Risk,
    /// How well the benefit is established.
    pub evidence: Evidence,
}

/// An ordered list of operations, each previewed and labelled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The items, applied in this order.
    pub items: Vec<PlanItem>,
}

impl Plan {
    /// Builds a plan by detecting the current state of every operation.
    ///
    /// Read-only. The preview reflects the system now; the executor detects
    /// again at apply time, so a stale preview never corrupts the journal.
    pub fn build(requests: Vec<OperationRequest>, ports: &Ports) -> Result<Self, PlanError> {
        let mut items = Vec::with_capacity(requests.len());
        for (index, request) in requests.into_iter().enumerate() {
            let prior = request
                .operation
                .detect(ports)
                .map_err(|error| PlanError::Detect { index, error })?;
            let (subject, target) = request.operation.describe_target();
            items.push(PlanItem {
                preview: Preview {
                    subject,
                    current: prior.describe(),
                    target,
                },
                operation: request.operation,
                risk: request.risk,
                evidence: request.evidence,
            });
        }
        Ok(Self { items })
    }

    /// The plan's risk: the maximum over its items; `Safe` for an empty plan.
    #[must_use]
    pub fn risk(&self) -> Risk {
        self.items
            .iter()
            .map(|item| item.risk)
            .max()
            .unwrap_or(Risk::Safe)
    }

    /// Whether the plan has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
