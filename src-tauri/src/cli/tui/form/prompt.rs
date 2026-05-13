use crate::prompt::Prompt;

use super::{FormFocus, FormMode, PromptMetaField, PromptMetaFormState, TextInput};

impl PromptMetaFormState {
    pub fn new(id: String, name: String) -> Self {
        let mut form = Self {
            mode: FormMode::Add,
            focus: FormFocus::Fields,
            field_idx: 0,
            editing: false,
            id: TextInput::new(id),
            name: TextInput::new(name),
            description: TextInput::new(""),
            initial_snapshot: Default::default(),
        };
        form.capture_initial_snapshot();
        form
    }

    pub fn from_prompt(prompt: &Prompt) -> Self {
        let mut form = Self {
            mode: FormMode::Edit {
                id: prompt.id.clone(),
            },
            focus: FormFocus::Fields,
            field_idx: 0,
            editing: false,
            id: TextInput::new(prompt.id.clone()),
            name: TextInput::new(prompt.name.clone()),
            description: TextInput::new(prompt.description.clone().unwrap_or_default()),
            initial_snapshot: Default::default(),
        };
        form.capture_initial_snapshot();
        form
    }

    fn capture_initial_snapshot(&mut self) {
        self.initial_snapshot = self.snapshot();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.snapshot() != self.initial_snapshot
    }

    pub fn fields(&self) -> Vec<PromptMetaField> {
        vec![
            PromptMetaField::Id,
            PromptMetaField::Name,
            PromptMetaField::Description,
        ]
    }

    pub fn input(&self, field: PromptMetaField) -> &TextInput {
        match field {
            PromptMetaField::Id => &self.id,
            PromptMetaField::Name => &self.name,
            PromptMetaField::Description => &self.description,
        }
    }

    pub fn input_mut(&mut self, field: PromptMetaField) -> &mut TextInput {
        match field {
            PromptMetaField::Id => &mut self.id,
            PromptMetaField::Name => &mut self.name,
            PromptMetaField::Description => &mut self.description,
        }
    }

    pub fn id_value(&self) -> String {
        self.id.value.trim().to_string()
    }

    pub fn name_value(&self) -> String {
        self.name.value.trim().to_string()
    }

    pub fn description_value(&self) -> Option<String> {
        let value = self.description.value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    fn snapshot(&self) -> (String, String, String) {
        (
            self.id_value(),
            self.name_value(),
            self.description.value.trim().to_string(),
        )
    }
}
