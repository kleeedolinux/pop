use pop_query::{BudgetError, BudgetTracker, CancellationToken, QueryBudget};

#[test]
fn cancellation_is_shared_and_monotonic() {
    let token = CancellationToken::new();
    let observer = token.clone();

    assert!(observer.check().is_ok());
    token.cancel();
    assert!(token.is_cancelled());
    assert!(observer.check().is_err());
}

#[test]
fn budgets_fail_before_publishing_unbounded_work() {
    let budget = QueryBudget::new(10, 32, 2);
    assert_eq!(budget.instruction_fuel(), 10);
    assert_eq!(budget.allocation_bytes(), 32);
    assert_eq!(budget.maximum_call_depth(), 2);
    let mut tracker = BudgetTracker::new(budget);

    assert_eq!(tracker.consume_instructions(6), Ok(()));
    assert_eq!(
        tracker.consume_instructions(5),
        Err(BudgetError::InstructionLimit)
    );
    assert_eq!(tracker.allocate(16), Ok(()));
    assert_eq!(tracker.allocate(17), Err(BudgetError::AllocationLimit));
    assert_eq!(tracker.enter_call(), Ok(()));
    assert_eq!(tracker.enter_call(), Ok(()));
    assert_eq!(tracker.enter_call(), Err(BudgetError::CallDepthLimit));
    assert_eq!(tracker.instructions(), 6);
    assert_eq!(tracker.allocation_bytes(), 16);
    assert_eq!(tracker.call_depth(), 2);
    assert_eq!(tracker.maximum_call_depth(), 2);
    tracker.exit_call().expect("balanced exit");
    tracker.exit_call().expect("balanced exit");
    assert_eq!(tracker.exit_call(), Err(BudgetError::UnbalancedCallExit));
}
