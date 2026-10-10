//! Fixture for a separate libatspi client. No synthetic semantic-tree reads.
#[cfg(target_os = "linux")]
fn main() {
    use bunny_ui::prelude::*;
    use bunny_ui_linux::{App, WindowSpec};
    use std::{cell::Cell, rc::Rc};

    if std::env::var_os("BUNNY_ACCESSIBILITY_PROBE").is_none() {
        println!(
            "Native AT-SPI fixture requires BUNNY_ACCESSIBILITY_PROBE=1 and the external client."
        );
        return;
    }
    #[derive(Clone)]
    struct Form {
        name: State<String>,
        value: State<String>,
        password: State<String>,
        presses: State<u32>,
        rows: State<Vec<u32>>,
        modal: State<bool>,
        close: Rc<dyn Fn()>,
    }
    impl Component for Form {
        fn body(self) -> impl View {
            vstack!(
                text("Native protocol witness"),
                text_field("Description", self.value.binding()).accessibility_label(self.name),
                text_field("Password", self.password.binding()).secret(true),
                button(text("Save"), move || {
                    assert_eq!(self.value.get(), "Dinner 👩‍🚀");
                    self.presses.add(1);
                    assert_eq!(self.presses.get(), 1);
                    self.name.set("Updated name".into());
                }),
                button(text("Open modal"), move || self.modal.set(true)),
                button(text("Remove row"), move || self.rows.set(vec![1])),
                button(text("Close form"), move || (self.close)()),
                for_each(
                    self.rows,
                    |id| id.to_string(),
                    move |id| {
                        let id = *id;
                        button(text(format!("Row {id}")), move || self.presses.add(id))
                    }
                ),
            )
            .padding()
            .sheet(self.modal.binding(), move |_| {
                erased(button(text("Dismiss modal"), move || self.modal.set(false)).padding())
            })
        }
    }
    let app = App::new();
    let main_id = Rc::new(Cell::new(None));
    let close: Rc<dyn Fn()> = Rc::new({
        let app = app.clone();
        let main_id = Rc::clone(&main_id);
        move || {
            app.close(
                main_id
                    .get()
                    .expect("form is mounted before its close action"),
            )
        }
    });
    let id = app.open(
        WindowSpec::titled("Bunny AT-SPI witness").size(480.0, 640.0),
        Rc::new(app.runtime()),
        Form {
            name: State::new("Description".into()),
            value: State::new("Lunch".into()),
            password: State::new("never-export-this".into()),
            presses: State::new(0),
            rows: State::new(vec![1, 2]),
            modal: State::new(false),
            close,
        },
    );
    main_id.set(Some(id));
    app.open(
        WindowSpec::titled("AT-SPI keeper").size(160.0, 120.0),
        Rc::new(app.runtime()),
        text("Keeper"),
    );
    app.run();
}

#[cfg(not(target_os = "linux"))]
fn main() {}
