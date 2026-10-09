//! An editable MVVM form. The domain has no UI imports or reactive types.
use bunny_ui::prelude::*;

mod domain {
    #[derive(Clone, Debug, PartialEq)]
    pub struct Profile {
        display_name: String,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct EmptyName;

    impl Profile {
        pub fn parse(name: &str) -> Result<Self, EmptyName> {
            let name = name.trim();
            if name.is_empty() {
                Err(EmptyName)
            } else {
                Ok(Self {
                    display_name: name.to_owned(),
                })
            }
        }

        pub fn display_name(&self) -> &str {
            &self.display_name
        }
    }
}

#[derive(Clone, Copy)]
struct ProfileModel {
    draft: State<String>,
    saved: State<domain::Profile>,
    error: State<Option<domain::EmptyName>>,
}

impl ProfileModel {
    fn new(profile: domain::Profile) -> Self {
        Self {
            draft: State::new(profile.display_name().to_owned()),
            saved: State::new(profile),
            error: State::new(None),
        }
    }

    fn save(self) {
        match self.draft.with(|draft| domain::Profile::parse(draft)) {
            Ok(profile) => {
                self.saved.set_if_changed(profile);
                self.error.set_if_changed(None);
            }
            Err(error) => {
                self.error.set_if_changed(Some(error));
            }
        }
    }

    fn status(self) -> Derived<String> {
        derived(move || {
            if self.error.get().is_some() {
                "Enter a display name before saving.".to_owned()
            } else if self.draft.with(|draft| {
                self.saved
                    .with(|saved| draft.trim() != saved.display_name())
            }) {
                "Unsaved changes".to_owned()
            } else {
                self.saved
                    .with(|saved| format!("Saved: {}", saved.display_name()))
            }
        })
    }
}

#[derive(Clone, Copy)]
struct ProfileEditor {
    vm: ProfileModel,
}

impl Component for ProfileEditor {
    fn body(self) -> impl View {
        vstack!(
            text("Display name"),
            text_field("Name", self.vm.draft.binding()),
            text(self.vm.status()),
            button(text("Save"), move || self.vm.save()),
        )
    }
}

fn main() -> Result<(), domain::EmptyName> {
    let runtime = Runtime::new();
    let editor = ProfileEditor {
        vm: ProfileModel::new(domain::Profile::parse("Ada")?),
    };
    println!("{}", runtime.render(&editor));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_saving_preserve_domain_validation_without_a_window() {
        let vm = ProfileModel::new(domain::Profile::parse("Ada").unwrap());
        let status = vm.status();
        vm.draft.binding().set("  Grace  ".into());
        assert_eq!(status.get(), "Unsaved changes");
        vm.save();
        assert_eq!(status.get(), "Saved: Grace");
        vm.draft.binding().set("   ".into());
        vm.save();
        assert_eq!(status.get(), "Enter a display name before saving.");
        assert_eq!(vm.saved.get().display_name(), "Grace");
        vm.draft.set("Linus".into());
        vm.save();
        assert_eq!(status.get(), "Saved: Linus");
    }
}
