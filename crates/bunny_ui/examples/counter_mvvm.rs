//! MVVM with a plain presentation struct, independent properties and commands.
extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct CounterModel {
    count: State<i32>,
    name: State<String>,
}

impl CounterModel {
    fn new() -> Self {
        Self {
            count: State::new(0),
            name: State::new("Ada".to_owned()),
        }
    }

    fn increment(self) {
        self.count.add(1);
    }

    fn greeting(self) -> Derived<String> {
        derived(move || format!("Hello, {}", self.name.get()))
    }
}

#[derive(Clone, Copy)]
struct Counter {
    vm: CounterModel,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text(self.vm.greeting()),
            text_field("Name", self.vm.name.binding()),
            text(self.vm.count),
            button(text("Increment"), move || self.vm.increment()),
        )
    }
}

fn main() {
    // No window, renderer backend or async runtime needed to exercise the view.
    let runtime = Runtime::new();
    // This model belongs to the application and is supplied to the view.
    let counter = Counter {
        vm: CounterModel::new(),
    };
    println!("{}", runtime.render(&counter));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_bindings_work_without_a_view() {
        let vm = CounterModel::new();
        vm.increment();
        vm.name.binding().set("Grace".to_owned());
        assert_eq!(vm.count.get(), 1);
        assert_eq!(vm.greeting().get(), "Hello, Grace");
    }
}
