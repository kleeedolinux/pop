use pop_foundation::TypeId;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InferenceVariableId(usize);

impl InferenceVariableId {
    #[must_use]
    pub const fn raw(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InferenceType {
    Known(TypeId),
    Variable(InferenceVariableId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InferenceError {
    Unsolved(InferenceVariableId),
    Conflict { expected: TypeId, found: TypeId },
    UnknownVariable(InferenceVariableId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VariableState {
    parent: InferenceVariableId,
    binding: Option<TypeId>,
}

#[derive(Clone, Debug, Default)]
pub struct InferenceContext {
    variables: Vec<VariableState>,
}

impl InferenceContext {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            variables: Vec::new(),
        }
    }

    #[must_use]
    pub fn new_variable(&mut self) -> InferenceVariableId {
        let id = InferenceVariableId(self.variables.len());
        self.variables.push(VariableState {
            parent: id,
            binding: None,
        });
        id
    }

    /// Adds an equality constraint between known types and inference variables.
    ///
    /// # Errors
    ///
    /// Returns [`InferenceError::Conflict`] for incompatible known bindings or
    /// [`InferenceError::UnknownVariable`] for an invalid variable ID.
    pub fn constrain_equal(
        &mut self,
        left: InferenceType,
        right: InferenceType,
    ) -> Result<(), InferenceError> {
        match (left, right) {
            (InferenceType::Known(left), InferenceType::Known(right)) => compatible(left, right),
            (InferenceType::Variable(variable), InferenceType::Known(known))
            | (InferenceType::Known(known), InferenceType::Variable(variable)) => {
                self.bind(variable, known)
            }
            (InferenceType::Variable(left), InferenceType::Variable(right)) => {
                self.unify(left, right)
            }
        }
    }

    /// Resolves a variable to its proven static type.
    ///
    /// # Errors
    ///
    /// Returns [`InferenceError::Unsolved`] instead of creating a dynamic type,
    /// or [`InferenceError::UnknownVariable`] for an invalid variable ID.
    pub fn resolve(&self, variable: InferenceVariableId) -> Result<TypeId, InferenceError> {
        let root = self.root(variable)?;
        self.state(root)?
            .binding
            .ok_or(InferenceError::Unsolved(variable))
    }

    fn bind(&mut self, variable: InferenceVariableId, known: TypeId) -> Result<(), InferenceError> {
        let root = self.root(variable)?;
        let state = self.state_mut(root)?;
        if let Some(binding) = state.binding {
            compatible(binding, known)
        } else {
            state.binding = Some(known);
            Ok(())
        }
    }

    fn unify(
        &mut self,
        left: InferenceVariableId,
        right: InferenceVariableId,
    ) -> Result<(), InferenceError> {
        let left = self.root(left)?;
        let right = self.root(right)?;
        if left == right {
            return Ok(());
        }
        let left_binding = self.state(left)?.binding;
        let right_binding = self.state(right)?.binding;
        if let (Some(left), Some(right)) = (left_binding, right_binding) {
            compatible(left, right)?;
        }
        let (parent, child) = if left < right {
            (left, right)
        } else {
            (right, left)
        };
        let binding = left_binding.or(right_binding);
        self.state_mut(child)?.parent = parent;
        self.state_mut(parent)?.binding = binding;
        Ok(())
    }

    fn root(
        &self,
        mut variable: InferenceVariableId,
    ) -> Result<InferenceVariableId, InferenceError> {
        loop {
            let parent = self.state(variable)?.parent;
            if parent == variable {
                return Ok(variable);
            }
            variable = parent;
        }
    }

    fn state(&self, variable: InferenceVariableId) -> Result<&VariableState, InferenceError> {
        self.variables
            .get(variable.0)
            .ok_or(InferenceError::UnknownVariable(variable))
    }

    fn state_mut(
        &mut self,
        variable: InferenceVariableId,
    ) -> Result<&mut VariableState, InferenceError> {
        self.variables
            .get_mut(variable.0)
            .ok_or(InferenceError::UnknownVariable(variable))
    }
}

fn compatible(expected: TypeId, found: TypeId) -> Result<(), InferenceError> {
    if expected == found {
        Ok(())
    } else {
        Err(InferenceError::Conflict { expected, found })
    }
}
