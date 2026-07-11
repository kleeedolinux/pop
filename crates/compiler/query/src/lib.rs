//! Incremental query cancellation and deterministic resource budgets.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Checks whether cancellation was requested.
    ///
    /// # Errors
    ///
    /// Returns [`Cancelled`] after this token or any clone is cancelled.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("query cancelled")
    }
}

impl Error for Cancelled {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryBudget {
    instruction_fuel: u64,
    allocation_bytes: u64,
    maximum_call_depth: u32,
}

impl QueryBudget {
    #[must_use]
    pub const fn new(
        instruction_fuel: u64,
        allocation_bytes: u64,
        maximum_call_depth: u32,
    ) -> Self {
        Self {
            instruction_fuel,
            allocation_bytes,
            maximum_call_depth,
        }
    }

    #[must_use]
    pub const fn instruction_fuel(self) -> u64 {
        self.instruction_fuel
    }

    #[must_use]
    pub const fn allocation_bytes(self) -> u64 {
        self.allocation_bytes
    }

    #[must_use]
    pub const fn maximum_call_depth(self) -> u32 {
        self.maximum_call_depth
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetError {
    InstructionLimit,
    AllocationLimit,
    CallDepthLimit,
    LiveValueLimit,
    OutputSizeLimit,
    DiagnosticLimit,
    UnbalancedCallExit,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "query budget exhausted: {self:?}")
    }
}

impl Error for BudgetError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetTracker {
    budget: QueryBudget,
    instructions: u64,
    allocations: u64,
    call_depth: u32,
    maximum_call_depth: u32,
}

impl BudgetTracker {
    #[must_use]
    pub const fn new(budget: QueryBudget) -> Self {
        Self {
            budget,
            instructions: 0,
            allocations: 0,
            call_depth: 0,
            maximum_call_depth: 0,
        }
    }

    #[must_use]
    pub const fn instructions(&self) -> u64 {
        self.instructions
    }

    #[must_use]
    pub const fn allocation_bytes(&self) -> u64 {
        self.allocations
    }

    #[must_use]
    pub const fn call_depth(&self) -> u32 {
        self.call_depth
    }

    #[must_use]
    pub const fn maximum_call_depth(&self) -> u32 {
        self.maximum_call_depth
    }

    /// Charges deterministic interpreter/query instructions.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::InstructionLimit`] before exceeding the limit.
    pub fn consume_instructions(&mut self, amount: u64) -> Result<(), BudgetError> {
        let Some(next) = self.instructions.checked_add(amount) else {
            return Err(BudgetError::InstructionLimit);
        };
        if next > self.budget.instruction_fuel {
            return Err(BudgetError::InstructionLimit);
        }
        self.instructions = next;
        Ok(())
    }

    /// Charges deterministic allocation bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::AllocationLimit`] before exceeding the limit.
    pub fn allocate(&mut self, bytes: u64) -> Result<(), BudgetError> {
        let Some(next) = self.allocations.checked_add(bytes) else {
            return Err(BudgetError::AllocationLimit);
        };
        if next > self.budget.allocation_bytes {
            return Err(BudgetError::AllocationLimit);
        }
        self.allocations = next;
        Ok(())
    }

    /// Enters one nested query/compile-time call.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::CallDepthLimit`] before exceeding the limit.
    pub fn enter_call(&mut self) -> Result<(), BudgetError> {
        let Some(next) = self.call_depth.checked_add(1) else {
            return Err(BudgetError::CallDepthLimit);
        };
        if next > self.budget.maximum_call_depth {
            return Err(BudgetError::CallDepthLimit);
        }
        self.call_depth = next;
        self.maximum_call_depth = self.maximum_call_depth.max(next);
        Ok(())
    }

    /// Leaves one nested query/compile-time call.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::UnbalancedCallExit`] at depth zero.
    pub fn exit_call(&mut self) -> Result<(), BudgetError> {
        let Some(next) = self.call_depth.checked_sub(1) else {
            return Err(BudgetError::UnbalancedCallExit);
        };
        self.call_depth = next;
        Ok(())
    }
}
