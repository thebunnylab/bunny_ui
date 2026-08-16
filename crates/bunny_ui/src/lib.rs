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
//!     fn body(self, _ctx: &Context) -> impl View {
//!         vstack!(
//!             text!("count: {}", self.count),
//!             button(text("increment"), move || self.count.add(1)),
//!         )
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
pub mod text_engine;
pub mod text_input;
pub mod view;
pub mod views;

/// `text!("Count: {}", self.count)` — o `format!` embutido do texto.
/// Exibir um `State` LÊ o valor: a dependência registra sozinha.
#[macro_export]
macro_rules! text {
    ($($arg:tt)*) => {
        $crate::views::text(::std::format!($($arg)*))
    };
}

/// `vstack!(a, b, c)` — os filhos sem os parênteses dobrados da tupla.
#[macro_export]
macro_rules! vstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::vstack(($($child,)+))
    };
}

/// `hstack!(a, b, c)` — ver [`vstack!`].
#[macro_export]
macro_rules! hstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::hstack(($($child,)+))
    };
}

/// `zstack!(a, b, c)` — ver [`vstack!`].
#[macro_export]
macro_rules! zstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::zstack(($($child,)+))
    };
}

pub mod prelude {
    pub use crate::erased::{CustomModifier, Erased, erased};
    pub use crate::{hstack, text, vstack, zstack};
    pub use crate::ext::ViewExt;
    pub use crate::layout::{Color, VisualProps};
    pub use crate::text_engine::{FontDesign, FontSpec, PixelFont, TextEngine, Weight};
    pub use crate::text_input::{CaretState, EditCommand};
    pub use crate::one_of::{OneOf3, OneOf4, OneOf5, OneOf6, OneOf7, OneOf8};
    pub use crate::runtime::{Edited, ImeSnapshot, Runtime};
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
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
                text("probe").on_change(
                    move || self.flag.get(),
                    false,
                    move |_, new| self.seen.update(|seen| seen.push(*new)),
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
            fn body(self, _ctx: &Context) -> impl View {
                (
                    text("a").on_change(
                        move || self.value.get(),
                        false,
                        move |_, new| self.first.set(*new),
                    ),
                    text("b").on_change(
                        move || self.value.get(),
                        false,
                        move |_, new| self.second.set(*new),
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
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
                // o publisher é recomputado a cada body — como no app real
                text("w").on_receive(self.store.updates(|value| *value), move |value| {
                    self.seen.borrow_mut().push(value)
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
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("count: {}", self.count.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Screen {
            title: Title,
        }

        impl Component for Screen {
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text(format!("count: {}", self.count.get())),
                    button(text("tap!"), move || self.count.update(|n| *n += 1)),
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
        // sem flexível no body, o root responde o natural, não o viewport —
        // proposta é oferta, não imposição. Natural = linha do título (16)
        // + botão com chrome (16 do label + 2×6 de padding embutido = 28)
        assert_eq!(title.size.height, 44.0);
        assert!(runtime.render(&tapper).contains("count: 1"));

        // e um clique num frame de view PULADA continua funcionando (a
        // ação é retida como os efeitos)
        let result = runtime.layout(&tapper, viewport);
        assert!(runtime.body_runs().is_empty(), "tudo do cache");
        let (path, _) = result.hits.last().unwrap().clone();
        assert!(runtime.activate(&path));
        assert!(runtime.render(&tapper).contains("count: 2"));
    }

    /// O par (runtime estabilizado, centro do botão) que os testes de
    /// ponteiro compartilham.
    fn pressable() -> (Runtime, TapperFixture, f64, f64) {
        use crate::layout::{Proposal, Size};

        let tapper = TapperFixture { count: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&tapper);
        let result =
            runtime.layout(&tapper, Proposal::exact(Size { width: 200.0, height: 100.0 }));
        let (_, rect) = result.hits.last().expect("o botão registra um alvo").clone();
        let cx = rect.origin.x + rect.size.width / 2.0;
        let cy = rect.origin.y + rect.size.height / 2.0;
        (runtime, tapper, cx, cy)
    }

    #[derive(Clone, Copy)]
    struct TapperFixture {
        count: State<i32>,
    }

    impl Component for TapperFixture {
        fn body(self, _ctx: &Context) -> impl View {
            vstack((
                text(format!("count: {}", self.count.get())),
                button(text("tap!"), move || self.count.update(|n| *n += 1)),
            ))
        }
    }

    #[test]
    fn hover_repaints_without_running_a_single_body() {
        use crate::layout::{DrawCommand, Proposal, Size};

        let (runtime, tapper, cx, cy) = pressable();
        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let cold = runtime.layout(&tapper, viewport);

        assert!(runtime.pointer_moved(cx, cy), "entrar no alvo muda o estado");
        let hot = runtime.layout(&tapper, viewport);
        assert!(runtime.body_runs().is_empty(), "hover repinta com ZERO bodies");

        // a LEI: frames byte-idênticos sob qualquer interação
        for (path, frame) in cold.frames.iter() {
            assert_eq!(Some(frame), hot.frames.get(path));
        }
        let backgrounds = |result: &crate::layout::LayoutResult| -> Vec<Color> {
            result
                .display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .collect()
        };
        assert_ne!(backgrounds(&cold), backgrounds(&hot), "a pintura muda");

        // 1px dentro do mesmo alvo: nada a repintar
        assert!(!runtime.pointer_moved(cx + 1.0, cy));
    }

    #[test]
    fn up_inside_fires_and_down_alone_does_not() {
        let (runtime, tapper, cx, cy) = pressable();

        assert!(runtime.pointer_pressed(cx, cy));
        assert!(
            runtime.render_stable(&tapper).contains("count: 0"),
            "down sozinho não dispara"
        );
        assert!(runtime.pointer_released(cx, cy).is_some(), "up-inside dispara");
        assert!(runtime.render_stable(&tapper).contains("count: 1"));
    }

    #[test]
    fn release_outside_never_fires() {
        let (runtime, tapper, cx, cy) = pressable();

        runtime.pointer_pressed(cx, cy);
        assert_eq!(runtime.pointer_released(199.0, 99.0), None, "soltou fora");
        runtime.pointer_pressed(199.0, 99.0);
        assert_eq!(runtime.pointer_released(cx, cy), None, "press fora, up dentro");
        assert!(runtime.render_stable(&tapper).contains("count: 0"));
    }

    #[test]
    fn drag_out_and_back_rearms_the_press() {
        let (runtime, tapper, cx, cy) = pressable();

        runtime.pointer_pressed(cx, cy);
        runtime.pointer_moved(199.0, 99.0);
        assert!(
            runtime.interaction().hovered.is_none(),
            "arrastar para fora solta o visual"
        );
        runtime.pointer_moved(cx, cy);
        assert_eq!(
            runtime.interaction().hovered,
            runtime.interaction().pressed,
            "voltar re-arma"
        );
        assert!(runtime.pointer_released(cx, cy).is_some());
        assert!(runtime.render_stable(&tapper).contains("count: 1"));
    }

    #[test]
    fn pointer_exited_clears_the_hover() {
        let (runtime, _tapper, cx, cy) = pressable();

        runtime.pointer_moved(cx, cy);
        assert!(runtime.pointer_exited());
        assert!(runtime.interaction().hovered.is_none());
        assert!(!runtime.pointer_exited(), "já limpo — nada a repintar");
    }

    #[test]
    fn style_modifiers_merge_into_one_styled() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = text("ab").corner_radius(5.0).background_color(Color::hex(0x123456));
        let result = runtime.layout(&view, Proposal::unspecified());

        let fills: Vec<(Color, f64)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { color, corner_radius, .. } => {
                    Some((*color, *corner_radius))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            fills,
            vec![(Color::hex(0x123456), 5.0)],
            "um Styled só — o raio arredonda ESTE fundo"
        );
    }

    #[test]
    fn the_nearest_background_wins_within_one_view() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = text("ab")
            .background_color(Color::hex(0x111111))
            .background_color(Color::hex(0x222222));
        let result = runtime.layout(&view, Proposal::unspecified());

        let fills: Vec<Color> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(
            fills,
            vec![Color::hex(0x111111)],
            "o mais próximo da view vence; um comando só"
        );
    }

    #[test]
    fn padding_and_background_order_mirror_the_api() {
        use crate::layout::{DrawCommand, Proposal, Rect};

        let runtime = Runtime::new();
        let outer = runtime.layout(
            &text("ab").padding_length(10.0).background_color(Color::BLACK),
            Proposal::unspecified(),
        );
        let inner = runtime.layout(
            &text("ab").background_color(Color::BLACK).padding_length(10.0),
            Proposal::unspecified(),
        );

        let fill_rect = |result: &crate::layout::LayoutResult| -> Rect {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { rect, .. } => Some(*rect),
                    _ => None,
                })
                .unwrap()
        };
        // fundo por fora do padding cobre a área acolchoada; por dentro,
        // só o conteúdo — o tamanho total não muda, muda quem pinta o quê
        assert_eq!(fill_rect(&outer).size.width, 36.0);
        assert_eq!(fill_rect(&inner).size.width, 16.0);
        assert_eq!(outer.size, inner.size);
    }

    #[test]
    fn visual_modifiers_print_their_suffixes() {
        let runtime = Runtime::new();
        let printed = runtime.render(
            &text("x")
                .background_color(Color::hex(0xAA69FB))
                .foreground_color(Color::hex(0x070510))
                .border(Color::hex(0x2A1B3F), 1.0)
                .corner_radius(6.0),
        );
        assert!(printed.contains("[.background(#AA69FB)]"), "{printed}");
        assert!(printed.contains("[.foregroundColor(#070510)]"), "{printed}");
        assert!(printed.contains("[.border(#2A1B3F, width: 1)]"), "{printed}");
        assert!(printed.contains("[.cornerRadius(6)]"), "{printed}");
    }

    #[test]
    fn button_chrome_shows_up_in_the_display_list() {
        use crate::layout::{DrawCommand, Proposal};

        #[derive(Clone, Copy)]
        struct One;

        impl Component for One {
            fn body(self, _ctx: &Context) -> impl View {
                button(text("go"), || {})
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&One);
        let result = runtime.layout(&One, Proposal::unspecified());

        // o fundo do chrome vem antes do texto do label, com os cantos
        let mut fill_before_text = false;
        let mut saw_text = false;
        for command in result.display.iter() {
            match command {
                DrawCommand::FillRect { corner_radius, .. } if !saw_text => {
                    assert_eq!(*corner_radius, 6.0);
                    fill_before_text = true;
                }
                DrawCommand::TextLine { .. } => saw_text = true,
                _ => {}
            }
        }
        assert!(fill_before_text && saw_text);

        // o hit-rect é o chrome inteiro: label + padding embutido
        let (_, rect) = result.hits.last().unwrap().clone();
        assert_eq!(rect.size.height, 28.0, "16 do label + 2×6");
        assert_eq!(rect.size.width, 44.0, "2 chars × 8 + 2×14");
    }

    #[test]
    fn wheel_scrolls_clamps_and_persists_by_identity() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Rows {
            flip: State<bool>,
        }

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                let _ = self.flip.get(); // lida: o set() invalida este body
                list(
                    (0..10).map(|index| index.to_string()).collect(),
                    |item: &String| item.clone(),
                    |item: &String| text(format!("row {item}")),
                )
            }
        }

        let rows = Rows { flip: State::new(false) };
        let runtime = Runtime::new();
        runtime.render_stable(&rows);
        let viewport = Proposal::exact(Size { width: 120.0, height: 100.0 });
        let result = runtime.layout(&rows, viewport);

        assert_eq!(result.scrolls.len(), 1, "a List é uma região com identidade");
        let path = result.scrolls[0].path.clone();

        // delta negativo (conteúdo para cima) → offset cresce
        assert!(runtime.wheel(10.0, 10.0, 0.0, -30.0));
        assert_eq!(runtime.scroll_offset(&path).y, 30.0);
        // clamp snapado no fim do curso: 10×16 − 100 = 60
        assert!(runtime.wheel(10.0, 10.0, 0.0, -500.0));
        assert_eq!(runtime.scroll_offset(&path).y, 60.0);
        assert!(!runtime.wheel(10.0, 10.0, 0.0, -1.0), "no fim do curso não há repaint");
        assert!(!runtime.wheel(500.0, 500.0, 0.0, -10.0), "fora de qualquer região");

        // o offset aplica no layout: a primeira linha de texto sobe 60
        let scrolled = runtime.layout(&rows, viewport);
        let first_line_y = scrolled
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(first_line_y, -60.0);

        // invalidação e re-render NÃO perdem a posição — restauração por
        // identidade estrutural
        rows.flip.set(true);
        runtime.render_stable(&rows);
        let after = runtime.layout(&rows, viewport);
        assert_eq!(runtime.scroll_offset(&path).y, 60.0);
        let line_y = after
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(line_y, -60.0);

        // rolagem programática vale no MESMO frame
        runtime.set_scroll_offset(&path, crate::layout::Point { x: 0.0, y: 8.0 });
        let programmatic = runtime.layout(&rows, viewport);
        let line_y = programmatic
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(line_y, -8.0);
    }

    #[test]
    fn a_text_field_edits_through_focus_and_the_binding() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text(format!("hello {}", self.name.get())),
                    text_field("Your name", self.name.binding()),
                ))
            }
        }

        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 100.0 });
        let result = runtime.layout(&form, viewport);

        // vazio: o placeholder pinta na cor própria, sem foco
        let has_placeholder = result.display.iter().any(|command| matches!(
            command,
            DrawCommand::TextLine { color, content, .. }
                if *color == Color::PLACEHOLDER && content == "Your name"
        ));
        assert!(has_placeholder);

        // clicar no campo foca (up-inside → editor → foco)
        let (field_path, rect) = result.hits.last().expect("o campo é alvo").clone();
        let (cx, cy) = (
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        runtime.pointer_pressed(cx, cy);
        assert_eq!(runtime.pointer_released(cx, cy), Some(field_path.clone()));
        assert_eq!(runtime.focused(), Some(field_path.clone()));

        // digitar flui pelo binding: o TÍTULO (outra view) vê a mudança
        assert!(runtime.key(EditCommand::Insert("Deco".into())).applied);
        let printed = runtime.render_stable(&form);
        assert!(printed.contains("hello Deco"), "{printed}");

        // o frame focado pinta caret e borda de foco
        let focused_frame = runtime.layout(&form, viewport);
        assert!(focused_frame.display.iter().any(|command| matches!(
            command,
            DrawCommand::FillRect { rect, color, .. }
                if *color == Color::BLACK && rect.size.width < 2.0
        )));
        assert!(focused_frame.display.iter().any(|command| matches!(
            command,
            DrawCommand::StrokeRect { color, .. } if *color == Color::FOCUS
        )));

        // edição continua: backspace come o "o"
        assert!(runtime.key(EditCommand::Backspace).applied);
        assert!(runtime.render_stable(&form).contains("hello Dec"));

        // copy/cut extraem pela saída (a ponte do clipboard)
        assert!(runtime.key(EditCommand::SelectAll).applied);
        assert_eq!(runtime.key(EditCommand::Copy).output.as_deref(), Some("Dec"));
        assert_eq!(runtime.key(EditCommand::Cut).output.as_deref(), Some("Dec"));
        assert!(!runtime.render_stable(&form).contains("hello Dec"), "o cut removeu");
        assert!(runtime.key(EditCommand::Insert("Dec".into())).applied);

        // clique fora tira o foco; teclar sem foco não faz nada
        runtime.pointer_pressed(239.0, 99.0);
        runtime.pointer_released(239.0, 99.0);
        assert_eq!(runtime.focused(), None);
        assert!(!runtime.key(EditCommand::Insert("x".into())).applied);
        assert!(runtime.render_stable(&form).contains("hello Dec"));
    }

    #[test]
    fn the_dream_snippet_compiles_and_reacts() {
        use crate::layout::{Proposal, Size};

        // O código que a LEI de ergonomia pede: sem `let this`, sem
        // `.get()`, sem `format!`, sem parênteses dobrados — e por baixo,
        // o MESMO pipeline incremental de sempre.
        #[derive(Clone, Copy)]
        struct Counter {
            count: State<i32>,
        }

        impl Component for Counter {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text!("Count: {}", self.count),
                    button(text("Tap"), move || self.count.add(1)),
                )
            }
        }

        let counter = Counter { count: State::new(0) };
        let runtime = Runtime::new();
        assert!(runtime.render_stable(&counter).contains("Count: 0"));

        let result =
            runtime.layout(&counter, Proposal::exact(Size { width: 200.0, height: 100.0 }));
        let (_, rect) = result.hits.last().unwrap().clone();
        let cx = rect.origin.x + rect.size.width / 2.0;
        let cy = rect.origin.y + rect.size.height / 2.0;
        runtime.pointer_pressed(cx, cy);
        runtime.pointer_released(cx, cy);

        // o Display do State LÊ — o clique invalida SÓ o Counter
        runtime.render(&counter);
        assert_eq!(runtime.body_runs(), vec!["Counter".to_string()]);
        assert!(runtime.render_stable(&counter).contains("Count: 1"));
    }

    #[test]
    fn ime_composition_flows_through_the_runtime() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("name", self.name.binding())
            }
        }

        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 40.0 });
        let result = runtime.layout(&form, viewport);
        let (path, rect) = result.hits.last().unwrap().clone();
        let _ = path;
        runtime.pointer_pressed(rect.origin.x + 4.0, rect.origin.y + 4.0);
        runtime.pointer_released(rect.origin.x + 4.0, rect.origin.y + 4.0);

        // composição viva: o texto entra MARCADO (sublinhado, no binding)
        let mark = EditCommand::SetMarked { text: "にほん".into(), caret_utf16: (3, 0) };
        assert!(runtime.key(mark).applied);
        assert!(runtime.render_stable(&form).contains("にほん"));
        let composing = runtime.layout(&form, viewport);
        let underline = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .filter(|command| matches!(
                    command,
                    DrawCommand::FillRect { rect, color, .. }
                        if *color == Color::BLACK && rect.size.height == 1.0
                ))
                .count()
        };
        assert_eq!(underline(&composing), 1, "a composição pinta sublinhada");

        // o snapshot fala UTF-16 com a plataforma
        let snapshot = runtime.ime_snapshot().expect("campo focado");
        assert_eq!(snapshot.marked, Some((0, 3)));
        assert_eq!(snapshot.selected, (3, 0), "caret colapsado no fim da composição");

        // o commit troca o marcado pelo texto final e o sublinhado some
        assert!(runtime.key(EditCommand::Insert("日本".into())).applied);
        let committed = runtime.render_stable(&form);
        assert!(committed.contains("日本"), "{committed}");
        assert!(!committed.contains("にほん"));
        let after = runtime.layout(&form, viewport);
        assert_eq!(underline(&after), 0);
        assert_eq!(runtime.ime_snapshot().unwrap().marked, None);
    }

    #[test]
    fn click_positions_the_caret_and_blink_toggles_it() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("name", self.name.binding())
            }
        }

        let form = Form { name: State::new("abcdef".to_string()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 40.0 });
        let result = runtime.layout(&form, viewport);
        let field = result.fields.first().expect("campo colocado").clone();

        // clique no meio do "abcdef" (PixelFont: 8px/char): entre c e d
        let x = field.text_origin.x + 3.0 * 8.0 + 2.0;
        let y = field.frame.origin.y + field.frame.size.height / 2.0;
        runtime.pointer_pressed(x, y);
        runtime.pointer_released(x, y);
        assert_eq!(runtime.focused(), Some(field.path.clone()));
        // digitar no ponto clicado prova a posição sem expor o índice
        runtime.key(EditCommand::Insert("X".into()));
        assert!(runtime.render_stable(&form).contains("abcXdef"));

        // blink: o tick alterna a pintura do caret sem tocar em nada mais
        let caret_count = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .filter(|command| matches!(
                    command,
                    DrawCommand::FillRect { rect, color, .. }
                        if *color == Color::BLACK && rect.size.width < 2.0
                ))
                .count()
        };
        let visible = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&visible), 1);
        assert!(runtime.blink(), "focado: o tick pede repaint");
        let hidden = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&hidden), 0, "meio-período apagado");
        assert!(runtime.blink());
        let back = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&back), 1);
        // digitar volta para sólido mesmo no meio-período apagado
        runtime.blink();
        runtime.key(EditCommand::Insert("!".into()));
        let typing = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&typing), 1, "caret ativo não pisca");
        // sem foco, o tick não pede repaint
        runtime.blur();
        assert!(!runtime.blink());
    }

    #[test]
    fn text_measures_through_the_engine() {
        use crate::layout::{Proposal, Size};
        use crate::text_engine::{LineMetrics, TextRaster};

        // um engine de 10px/char prova a borda plugável: o frame muda sem
        // NENHUM componente saber qual engine está ativo
        struct Wide;
        impl TextEngine for Wide {
            fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
                LineMetrics {
                    width: text.chars().count() as f64 * 10.0,
                    ascent: 15.0,
                    descent: 5.0,
                }
            }
            fn raster_line(
                &self,
                _text: &str,
                _font: &FontSpec,
                _color: Color,
                _scale: usize,
            ) -> Option<TextRaster> {
                None
            }
        }

        let runtime = Runtime::new().text_engine(Rc::new(Wide));
        let result = runtime.layout(&text("abcd"), Proposal::unspecified());
        assert_eq!(result.size, Size { width: 40.0, height: 20.0 });
    }

    #[test]
    fn font_style_inherits_and_overrides() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = vstack((text("big"), text("small").font(Font::Caption))).font(Font::Title);
        let result = runtime.layout(&view, Proposal::unspecified());

        let sizes: Vec<f64> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { font, .. } => Some(font.size),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![22.0, 10.0], "Title herdado; Caption vence no filho");
    }

    #[test]
    fn paint_puts_ink_where_the_layout_put_the_text() {
        use crate::layout::Size;

        #[derive(Clone, Copy)]
        struct Badge {
            n: State<i32>,
        }

        impl Component for Badge {
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
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
