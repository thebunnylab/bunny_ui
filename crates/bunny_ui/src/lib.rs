//! `bunny_ui` — a camada tipada sobre o motor do `motor`.
//!
//! O mesmo runtime instrumentado (árvore de render, `render_stable`,
//! efeitos por site), agora monomórfico: views são valores genéricos
//! (`VStack<(Text, Button<…>)>`), `body` devolve `impl View` e o
//! apagamento vive só nas bordas de dinamismo real — [`Erased`] para
//! sheets e `ViewModifier`s, a fila de efeitos e os slots por site.
//!
//! ```ignore
//! #[derive(Clone, Copy)]
//! struct Counter {
//!     count: State<i32>,
//! }
//!
//! impl Component for Counter {
//!     fn body(&self, _ctx: &Context) -> impl View {
//!         let this = *self;
//!         vstack((
//!             text(format!("count: {}", self.count.get())),
//!             button(text("increment"), move || this.count.update(|c| *c += 1)),
//!         ))
//!     }
//! }
//! ```
//!
//! Três garantias desta camada, além do motor:
//!
//! - `State<T>` é `Copy` — views só de estado derivam `Copy` e closures
//!   capturam `self` sem cerimônia, como structs Swift;
//! - os sites de `on_change`/`on_receive` saem de `#[track_caller]` — cada
//!   callsite é seu próprio slot, sem string manual;
//! - aridade no tipo ([`Single`]/[`Many`]) — modifier em tupla crua não
//!   compila, em vez de decorar o nó errado em silêncio.
//!
//! [`Erased`]: crate::erased::Erased
//! [`Single`]: crate::view::Single
//! [`Many`]: crate::view::Many

#![forbid(unsafe_code)]

pub mod effects;
pub mod erased;
pub mod ext;
pub mod layout;
pub mod modifier;
pub mod one_of;
pub mod raster;
mod reconciler;
pub mod runtime;
pub mod state_ext;
pub mod view;
pub mod views;

pub mod prelude {
    pub use crate::erased::{CustomModifier, Erased, erased};
    pub use crate::ext::ViewExt;
    pub use crate::one_of::{OneOf3, OneOf4, OneOf5, OneOf6, OneOf7, OneOf8};
    pub use crate::runtime::Runtime;
    pub use crate::state_ext::{BindingExt, StateExt};
    pub use crate::view::{Component, Either, Many, Single, UnaryView, View};
    pub use crate::views::*;

    // O motor, re-exportado: o app só precisa desta prelude. Nomes
    // nominais do port espelhado não atravessam a borda pública.
    pub use std::rc::Rc;
    pub use motor::combine::{AnyPublisher, IntoPublisher, PassthroughSubject, Store};
    pub use motor::loadable::{Loadable, LoadableSubject, LoadError};
    pub use motor::runtime::Site;
    pub use motor::state::{
        Binding, Context, Environment, EnvironmentValues, FromEnvironment, Locale, ProvidesQueries,
        State,
    };
    pub use motor::views::{
        ContentMode, Edge, Font, ListStyle, NavigationPath, ProgressViewStyle, Query,
        TextAlignment,
    };
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use std::cell::RefCell;

    #[test]
    fn renders_the_same_tree_through_the_facade() {
        struct Cell;

        impl Component for Cell {
            fn body(&self, _ctx: &Context) -> impl View {
                vstack((
                    text("United States").font(Font::Title),
                    text("Population 125000000").font(Font::Caption),
                ))
                .alignment(HorizontalAlignment::Leading)
                .padding()
            }
        }

        impl Clone for Cell {
            fn clone(&self) -> Self {
                Cell
            }
        }

        let printed = Runtime::new().render_stable(&Cell);
        assert!(printed.contains("Cell"));
        assert!(printed.contains("VStack (alignment: .leading) [.padding()]"));
        assert!(printed.contains("Text(\"United States\") [.font(.title)]"));
    }

    #[test]
    fn state_handles_are_copy_so_views_can_be_too() {
        // O handle é Copy: cada closure captura a sua cópia implícita —
        // nenhum `let this = self.clone()` nomeado por papel. (As views de
        // estado derivam `Copy` inteiras; ver os testes de on_change.)
        let count = State::new(0);
        let increment = button(text("increment"), move || count.update(|c| *c += 1));
        increment.tap();
        increment.tap();
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn on_change_fires_only_when_the_value_moves() {
        #[derive(Clone, Copy)]
        struct Probe {
            flag: State<bool>,
            seen: State<Vec<bool>>,
        }

        impl Component for Probe {
            fn body(&self, _ctx: &Context) -> impl View {
                let this = *self;
                text("probe").on_change(
                    move || this.flag.get(),
                    false,
                    move |_, new| this.seen.update(|seen| seen.push(*new)),
                )
            }
        }

        let probe = Probe {
            flag: State::new(false),
            seen: State::new(Vec::new()),
        };
        let runtime = Runtime::new();

        // initial: false → o slot apenas aprende o valor, nada dispara
        runtime.render_stable(&probe);
        assert!(probe.seen.get().is_empty());

        // o valor anda → dispara uma vez, e estabiliza
        probe.flag.set(true);
        runtime.render_stable(&probe);
        assert_eq!(probe.seen.get(), vec![true]);
    }

    #[test]
    fn distinct_callsites_get_distinct_slots() {
        // Dois `on_change` do mesmo tipo, sem site manual: `#[track_caller]`
        // dá um slot para cada linha — nenhum vaza no do outro.
        #[derive(Clone, Copy)]
        struct Pair {
            value: State<i32>,
            first: State<i32>,
            second: State<i32>,
        }

        impl Component for Pair {
            fn body(&self, _ctx: &Context) -> impl View {
                let this = *self;
                (
                    text("a").on_change(
                        move || this.value.get(),
                        false,
                        move |_, new| this.first.set(*new),
                    ),
                    text("b").on_change(
                        move || this.value.get(),
                        false,
                        move |_, new| this.second.set(*new),
                    ),
                )
            }
        }

        let pair = Pair {
            value: State::new(0),
            first: State::new(-1),
            second: State::new(-1),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&pair);
        pair.value.set(7);
        runtime.render_stable(&pair);
        assert_eq!(pair.first.get(), 7);
        assert_eq!(pair.second.get(), 7);
    }

    #[test]
    fn row_state_follows_the_item_key_and_dies_with_it() {
        // A prova das três semânticas de posse: estado de row (a) revive
        // entre renders, (b) segue a chave numa reordenação, (c) morre com
        // a identidade e volta zerado num remount. O log de onAppear é o
        // detector: um mount = um appear.
        #[derive(Clone)]
        struct LoadRow {
            name: String,
            loaded: State<bool>,
            appeared: Rc<RefCell<Vec<String>>>,
        }

        impl Component for LoadRow {
            fn body(&self, _ctx: &Context) -> impl View {
                if self.loaded.get() {
                    Either::First(text(format!("{} ready", self.name)))
                } else {
                    let loaded = self.loaded;
                    let log = self.appeared.clone();
                    let name = self.name.clone();
                    Either::Second(text(format!("{} loading", self.name)).on_appear(move || {
                        log.borrow_mut().push(name.clone());
                        loaded.set(true);
                    }))
                }
            }
        }

        #[derive(Clone)]
        struct Board {
            items: State<Vec<&'static str>>,
            appeared: Rc<RefCell<Vec<String>>>,
        }

        impl Component for Board {
            fn body(&self, _ctx: &Context) -> impl View {
                let appeared = self.appeared.clone();
                list(
                    self.items.get(),
                    |item| item.to_string(),
                    move |item| LoadRow {
                        name: item.to_string(),
                        // construído DENTRO da row: ancora na chave do item
                        loaded: State::new(false),
                        appeared: appeared.clone(),
                    },
                )
            }
        }

        let board = Board {
            items: State::new(vec!["A", "B"]),
            appeared: Rc::new(RefCell::new(Vec::new())),
        };
        let runtime = Runtime::new();

        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready") && printed.contains("B ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B"]);

        // reordenar não zera: o estado seguiu a chave, nenhum appear novo
        board.items.set(vec!["B", "A"]);
        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready") && printed.contains("B ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B"]);

        // remover desmonta; recolocar é mount novo — estado zerado, appear de novo
        board.items.set(vec!["B"]);
        runtime.render_stable(&board);
        board.items.set(vec!["B", "A"]);
        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B", "A"]);
    }

    #[test]
    fn set_dirties_only_the_views_that_read() {
        #[derive(Clone, Copy)]
        struct Digit {
            n: State<i32>,
        }

        impl Component for Digit {
            fn body(&self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(&self, _ctx: &Context) -> impl View {
                vstack((self.a, self.b))
            }
        }

        let duo = Duo {
            a: Digit { n: State::new(0) },
            b: Digit { n: State::new(0) },
        };
        let runtime = Runtime::new();
        runtime.render_stable(&duo);

        // escrever no estado de `a` suja SÓ quem o leu — o Digit da posição
        // #0 — nunca o irmão. É a invalidação fina que o motor real usará
        // para re-rodar apenas os bodies atingidos.
        duo.a.n.set(1);
        let dirty = runtime.take_dirty();
        assert_eq!(dirty.len(), 1, "exatamente uma view suja: {dirty:?}");
        assert!(dirty[0].contains("#0"), "a posição da tupla identifica o irmão: {dirty:?}");
        assert!(dirty[0].ends_with("Digit"));
    }

    #[test]
    fn only_the_dirty_body_reruns_and_the_rest_comes_from_cache() {
        #[derive(Clone, Copy)]
        struct Digit {
            n: State<i32>,
        }

        impl Component for Digit {
            fn body(&self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(&self, _ctx: &Context) -> impl View {
                vstack((self.a, self.b))
            }
        }

        let duo = Duo {
            a: Digit { n: State::new(0) },
            b: Digit { n: State::new(0) },
        };
        let runtime = Runtime::new();
        runtime.render_stable(&duo);

        // um set no estado de `a`: o pai (Duo) fica PULADO — só o Digit #0
        // re-roda, isolado, a partir do valor retido; o irmão vem do cache
        duo.a.n.set(5);
        let printed = runtime.render(&duo);
        assert_eq!(runtime.body_runs(), vec!["Duo/#0/Digit".to_string()]);
        assert!(printed.contains("Text(\"5\")"));
        assert!(printed.contains("Text(\"0\")"), "o irmão intocado, do cache");

        // e o pass sem sujeira nenhuma não roda body algum
        let printed = runtime.render(&duo);
        assert!(runtime.body_runs().is_empty());
        assert!(printed.contains("Text(\"5\")"));

        // oráculo: o incremental imprime byte a byte o que o full imprime
        let incremental = runtime.render(&duo);
        let full = runtime.render_full(&duo);
        assert_eq!(incremental, full);
    }

    #[test]
    fn store_reads_in_the_body_are_dependencies_too() {
        // Granularidade de objeto: quem leu `store.value()` no body depende
        // do store inteiro — `send` re-roda a view, mesmo sem State no meio
        // (o caso sheet/blur: bindings despachados leem o store direto).
        #[derive(Clone)]
        struct Badge {
            store: Store<i32>,
        }

        impl Component for Badge {
            fn body(&self, _ctx: &Context) -> impl View {
                text(format!("badge {}", self.store.value()))
            }
        }

        let badge = Badge {
            store: Store::new(1),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&badge);

        badge.store.send(2);
        let printed = runtime.render(&badge);
        assert_eq!(runtime.body_runs(), vec!["Badge".to_string()]);
        assert!(printed.contains("badge 2"));
    }

    #[test]
    fn on_receive_delivers_once_per_value_change() {
        #[derive(Clone)]
        struct Watcher {
            store: Store<i32>,
            seen: Rc<RefCell<Vec<i32>>>,
        }

        impl Component for Watcher {
            fn body(&self, _ctx: &Context) -> impl View {
                let this = self.clone();
                // o publisher é recomputado a cada body — como no app real
                text("w").on_receive(self.store.updates(|value| *value), move |value| {
                    this.seen.borrow_mut().push(value)
                })
            }
        }

        let watcher = Watcher {
            store: Store::new(1),
            seen: Rc::new(RefCell::new(Vec::new())),
        };
        let runtime = Runtime::new();

        // dois render_stable completos: o valor inicial entrega UMA vez,
        // por menor que seja a vida do publisher recriado por body
        runtime.render_stable(&watcher);
        runtime.render_stable(&watcher);
        assert_eq!(*watcher.seen.borrow(), vec![1]);

        // o valor anda → entrega de novo
        watcher.store.send(5);
        runtime.render_stable(&watcher);
        assert_eq!(*watcher.seen.borrow(), vec![1, 5]);
    }

    #[test]
    fn a_real_view_tree_lays_out_through_the_runtime() {
        use crate::layout::{LINE_H, Proposal};

        // A tela: título + spacer + botão, num viewport de 200×100 — o
        // caminho inteiro (body-eval → árvore de layout → retenção →
        // expansão → frames) numa view com estado de verdade.
        #[derive(Clone, Copy)]
        struct Title {
            count: State<i32>,
        }

        impl Component for Title {
            fn body(&self, _ctx: &Context) -> impl View {
                text(format!("count: {}", self.count.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Screen {
            title: Title,
        }

        impl Component for Screen {
            fn body(&self, _ctx: &Context) -> impl View {
                let count = self.title.count;
                vstack((
                    self.title,
                    spacer(),
                    button(text("increment"), move || count.update(|n| *n += 1)),
                ))
            }
        }

        let screen = Screen { title: Title { count: State::new(0) } };
        let runtime = Runtime::new();
        runtime.render_stable(&screen);

        let viewport = Proposal { width: Some(200.0), height: Some(100.0) };
        let result = runtime.layout(&screen, viewport);

        assert_eq!(result.size.height, 100.0);
        let screen_frame = result.frames.get("Screen").unwrap();
        assert_eq!(screen_frame.size.height, 100.0);
        let title = result.frames.get("Screen/#0/Title").unwrap();
        assert_eq!(title.origin.y, 0.0);
        assert_eq!(title.size.height, LINE_H);

        // muda o estado → só o Title re-roda (invalidação fina) e o layout
        // seguinte reflete o texto novo — com o RESTO vindo do cache
        screen.title.count.set(42);
        let result = runtime.layout(&screen, viewport);
        assert_eq!(runtime.body_runs(), vec!["Screen/#0/Title".to_string()]);
        let title = result.frames.get("Screen/#0/Title").unwrap();
        // "count: 42" = 9 chars × 8px — o frame acompanha o conteúdo
        assert_eq!(title.size.width, 72.0);
    }

    #[test]
    fn a_click_routes_through_hit_test_to_the_action_and_repaints() {
        use crate::layout::{Proposal, Size, hit_test};

        #[derive(Clone, Copy)]
        struct Tapper {
            count: State<i32>,
        }

        impl Component for Tapper {
            fn body(&self, _ctx: &Context) -> impl View {
                let this = *self;
                vstack((
                    text(format!("count: {}", self.count.get())),
                    button(text("tap!"), move || this.count.update(|n| *n += 1)),
                ))
            }
        }

        let tapper = Tapper { count: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&tapper);

        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let result = runtime.layout(&tapper, viewport);

        // o botão está nos alvos de hit-test; um "clique" no meio dele
        // resolve para a chave da ação
        let (path, rect) = result.hits.last().expect("o botão registra um alvo").clone();
        let key = hit_test(
            &result.hits,
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        )
        .expect("o clique acerta o botão");
        assert_eq!(key, path);

        // clique fora não acerta nada
        assert!(hit_test(&result.hits, 199.0, 99.0).is_none());

        // disparar a ação muda o estado; o próximo frame re-roda SÓ o
        // Tapper e o layout reflete o texto novo — o ciclo vivo, headless
        assert!(runtime.activate(key));
        let result = runtime.layout(&tapper, viewport);
        assert_eq!(runtime.body_runs(), vec!["Tapper".to_string()]);
        let title = result.frames.get("Tapper").unwrap();
        // sem flexível no body, o root responde o natural (2 linhas), não
        // o viewport — proposta é oferta, não imposição
        assert_eq!(title.size.height, 32.0);
        assert!(runtime.render(&tapper).contains("count: 1"));

        // e um clique num frame de view PULADA continua funcionando (a
        // ação é retida como os efeitos)
        let result = runtime.layout(&tapper, viewport);
        assert!(runtime.body_runs().is_empty(), "tudo do cache");
        let (path, _) = result.hits.last().unwrap().clone();
        assert!(runtime.activate(&path));
        assert!(runtime.render(&tapper).contains("count: 2"));
    }

    #[test]
    fn paint_puts_ink_where_the_layout_put_the_text() {
        use crate::layout::Size;

        #[derive(Clone, Copy)]
        struct Badge {
            n: State<i32>,
        }

        impl Component for Badge {
            fn body(&self, _ctx: &Context) -> impl View {
                text(format!("n: {}", self.n.get()))
            }
        }

        let badge = Badge { n: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&badge);

        let size = Size { width: 64.0, height: 16.0 };
        let bitmap = runtime.paint(&badge, size);

        let white = bitmap.pixel(63, 15).unwrap();
        let ink_count = |bitmap: &crate::raster::Bitmap| {
            (0..16)
                .flat_map(|y| (0..64).map(move |x| (x, y)))
                .filter(|&(x, y)| bitmap.pixel(x, y) != Some(white))
                .count()
        };
        let before = ink_count(&bitmap);
        assert!(before > 0, "o texto pinta tinta");

        // o estado muda → re-render incremental → o bitmap muda junto
        badge.n.set(42);
        let after = ink_count(&runtime.paint(&badge, size));
        assert!(after > before, "\"n: 42\" tem mais tinta que \"n: 0\"");
    }

    #[test]
    fn state_and_binding_read_through_get() {
        let state = State::new(7);
        assert_eq!(state.get(), 7);

        let binding = state.binding();
        binding.set(9);
        assert_eq!(binding.get(), 9);
        assert_eq!(state.get(), 9);
    }

    #[test]
    fn tuples_option_and_one_of_compose_statically() {
        #[derive(Clone)]
        struct Row;

        impl Component for Row {
            fn body(&self, _ctx: &Context) -> impl View {
                // Option None não imprime; Either/OneOf escolhem o braço em
                // compile time — o tipo é a soma, o discriminante é runtime
                vstack((
                    None::<Text>,
                    text("kept"),
                    Either::<Text, Text>::First(text("first")),
                    OneOf3::<Text, Text, Text>::C(text("third")),
                ))
            }
        }

        let printed = Runtime::new().render_stable(&Row);
        assert!(printed.contains("Row"));
        assert!(printed.contains("Text(\"kept\")"));
        assert!(printed.contains("Text(\"first\")"));
        assert!(printed.contains("Text(\"third\")"));
        assert!(!printed.contains("TupleView"));
    }
}
