//! Modificadores de view — a `ViewExt` idiomática.
//!
//! Cada método devolve um [`Modified<Self>`] tipado: encadear `.font(…)`
//! não aloca nada nem apaga o tipo — a cadeia inteira vira um valor
//! monomórfico conhecido em compile time.
//!
//! A trait só existe para `Arity = Single` (um nó): modifier em tupla crua
//! não compila — ver [`Modified`].
//!
//! Os `site`s de `on_change`/`on_receive` — a identidade que o slot de
//! change-detection precisa entre renders — saem de `#[track_caller]`:
//! cada ponto de chamada é seu próprio site, sem string manual para
//! inventar (nem colidir). Quando a mesma chamada é compartilhada por um
//! helper e precisa se distinguir por uso, há as variantes `_keyed`.

use std::panic::Location;
use std::rc::Rc;

use motor::combine::IntoPublisher;
use motor::runtime::Site;
use motor::state::{Binding, Context, ProvidesQueries};
use motor::views::Query;

use crate::effects;
use crate::erased::Erased;
use crate::layout::Color;
use crate::modifier::{Modified, Modifier};
use crate::view::{Single, View, render_line, short_type_name};
use crate::views::Alignment;
use motor::views::{ContentMode, Edge, Font, ListStyle, ProgressViewStyle, TextAlignment};

pub trait ViewExt: View<Arity = Single> + Sized {
    // MARK: - Formatação

    /// `.font(.title)`
    fn font(self, font: Font) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Font(font),
        }
    }

    /// `.bold()`
    fn bold(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Bold,
        }
    }

    /// `.padding()`
    fn padding(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Padding,
        }
    }

    /// `.padding(12)` — uniforme com medida explícita.
    fn padding_length(self, length: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::PaddingLength(length),
        }
    }

    /// `.padding(.bottom, 40)`
    fn padding_edge(self, edge: Edge, length: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::PaddingEdge(edge, length),
        }
    }

    /// `.frame(width: 120, height: 80)`
    fn frame(self, width: f64, height: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameWH(width, height),
        }
    }

    /// `.frame(maxWidth: .infinity, maxHeight: 60, alignment: .leading)`
    fn frame_max(self, max_width: f64, max_height: f64, alignment: Alignment) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameMax(max_width, max_height, alignment),
        }
    }

    /// `.navigationTitle("…")`
    fn navigation_title(self, title: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationTitle(title.into()),
        }
    }

    /// `.navigationBarTitle("…")`
    fn nav_bar_title(self, title: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationBarTitle(title.into()),
        }
    }

    /// `.listStyle(.grouped)`
    fn list_style(self, style: ListStyle) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ListStyle(style),
        }
    }

    /// `.progressViewStyle(.circular)`
    fn progress_style(self, style: ProgressViewStyle) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ProgressViewStyle(style),
        }
    }

    /// `.multilineTextAlignment(.center)`
    fn multiline_text_alignment(self, alignment: TextAlignment) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::MultilineTextAlignment(alignment),
        }
    }

    /// `.aspectRatio(contentMode: .fit)`
    fn aspect_ratio(self, mode: ContentMode) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AspectRatio(mode),
        }
    }

    /// `.resizable()`
    fn resizable(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Resizable,
        }
    }

    /// `.blur(radius: 10)`
    fn blur(self, radius: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Blur(radius),
        }
    }

    /// `.ignoresSafeArea()`
    fn ignores_safe_area(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::IgnoresSafeArea,
        }
    }

    /// `.background { … }` — descreve o conteúdo sem montá-lo.
    fn background<C: View<Arity = Single>>(self, content: C) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Background(render_line(&content)),
        }
    }

    /// `.hidden()`
    fn hidden(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Hidden,
        }
    }

    // MARK: - Visuais (propriedades semânticas do nó — `Styled` na cena)

    /// `.background(Color.red)` — cor sólida como propriedade do nó (o
    /// `.background { view }` de conteúdo é o outro método).
    fn background_color(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::BackgroundColor(color),
        }
    }

    /// `.foregroundColor(.secondary)` — herdado pelo texto abaixo.
    fn foreground_color(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ForegroundColor(color),
        }
    }

    /// `.border(Color.gray, width: 1)` — moldura para dentro da aresta.
    fn border(self, color: Color, width: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Border(color, width),
        }
    }

    /// `.cornerRadius(8)` — arredonda o fundo DESTE nó.
    fn corner_radius(self, radius: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::CornerRadius(radius),
        }
    }

    // MARK: - Interação

    /// `.onTapGesture { … }` — no runtime headless dispara no render.
    fn on_tap(self, action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnTapGesture(Rc::new(action)),
        }
    }

    /// `.onAppear { … }` — dispara no render (paridade do motor).
    fn on_appear(self, action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnAppear(Rc::new(action)),
        }
    }

    /// `.onChange(of:initial:) { old, new in … }`
    ///
    /// O site do slot de change-detection é o próprio callsite
    /// (`#[track_caller]`). Se a chamada mora num helper reusado e cada uso
    /// precisa do seu slot, use [`ViewExt::on_change_keyed`].
    #[track_caller]
    fn on_change<V: Clone + PartialEq + 'static>(
        self,
        of: impl Fn() -> V + 'static,
        initial: bool,
        action: impl Fn(&V, &V) + 'static,
    ) -> Modified<Self> {
        self.on_change_keyed(Location::caller(), of, initial, action)
    }

    /// `.onChange` com site explícito — para helpers que emitem o mesmo
    /// callsite para usos que precisam de slots distintos.
    fn on_change_keyed<V: Clone + PartialEq + 'static>(
        self,
        site: impl Into<Site>,
        of: impl Fn() -> V + 'static,
        initial: bool,
        action: impl Fn(&V, &V) + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "onChange",
                detail: "()",
                effect: effects::change_effect(site.into(), of, initial, action),
            },
        }
    }

    /// `.onReceive(publisher) { value in … }`
    ///
    /// O site (`#[track_caller]`) retém a subscription entre renders — a
    /// identidade de view do SwiftUI. Sem ele, cada re-render criaria um
    /// publisher novo (célula de dedup zerada) que entregaria o valor atual
    /// de novo, reportando "mudança" a cada pump e nunca estabilizando.
    #[track_caller]
    fn on_receive<V: Clone + PartialEq + 'static>(
        self,
        publisher: impl IntoPublisher<V>,
        action: impl Fn(V) + 'static,
    ) -> Modified<Self> {
        self.on_receive_keyed(Location::caller(), publisher, action)
    }

    /// `.onReceive` com site explícito — mesmo caso do
    /// [`ViewExt::on_change_keyed`].
    fn on_receive_keyed<V: Clone + PartialEq + 'static>(
        self,
        site: impl Into<Site>,
        publisher: impl IntoPublisher<V>,
        action: impl Fn(V) + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "onReceive",
                detail: "()",
                effect: effects::receive_effect(site.into(), publisher.into_publisher(), action),
            },
        }
    }

    /// `.searchable(text: $searchText)`
    fn searchable(self, _text: Binding<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Searchable,
        }
    }

    /// `.refreshable { … }` — gesto de usuário; inert headless.
    fn refreshable(self, _action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Refreshable,
        }
    }

    /// `.sheet(isPresented: $flag) { … }` — o conteúdo é a borda apagada:
    /// monta só quando apresentada (feche com [`erased`]).
    ///
    /// [`erased`]: crate::erased::erased
    fn sheet(
        self,
        is_presented: Binding<bool>,
        content: impl Fn(&Context) -> Erased + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Sheet {
                is_presented,
                content: Rc::new(content),
            },
        }
    }

    /// `.toolbar { … }` — inert no runtime fake, como no motor.
    fn toolbar(self, _items: impl View) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Toolbar,
        }
    }

    // MARK: - Dados & environment

    /// `.inject(diContainer)`
    fn inject<T: 'static>(self, container: Rc<T>) -> Modified<Self> {
        let detail = format!("({})", short_type_name::<T>());
        Modified {
            base: self,
            modifier: Modifier::EnvSet {
                name: "inject",
                detail,
                set: Rc::new(move |values: &mut motor::state::EnvironmentValues| {
                    values.injected = Some(container.clone());
                }),
            },
        }
    }

    /// `.modelContainer(container)`
    fn model_container<T: ProvidesQueries + 'static>(self, container: Rc<T>) -> Modified<Self> {
        let source = container.querySource();
        Modified {
            base: self,
            modifier: Modifier::EnvSet {
                name: "modelContainer",
                detail: "(…)".into(),
                set: Rc::new(move |values: &mut motor::state::EnvironmentValues| {
                    values.querySource = Some(source.clone());
                }),
            },
        }
    }

    /// `.modifier(RootViewAppearance(…))` — re-aplicado a cada render.
    fn modifier(self, custom: impl crate::erased::CustomModifier + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Custom(Rc::new(custom)),
        }
    }

    /// `.query(searchText:results:) { search in Query(…) }` — o @Query fake.
    fn query<T: Clone + PartialEq + 'static>(
        self,
        search_text: String,
        results: Binding<Vec<T>>,
        build: impl Fn(String) -> Query<T> + 'static,
    ) -> Modified<Self> {
        let effect: motor::state::EffectFn = Rc::new(move |ctx: &Context| {
            let Some(source) = ctx.values.querySource.clone() else {
                return false;
            };
            let Some(any) = source(std::any::type_name::<T>()) else {
                return false;
            };
            let Ok(storage) = any.downcast::<std::cell::RefCell<Vec<T>>>() else {
                return false;
            };

            let query = build(search_text.clone());
            let items: Vec<T> = storage
                .borrow()
                .iter()
                .filter(|item| (query.filter)(item))
                .cloned()
                .collect();
            let mut keyed: Vec<(String, T)> = items
                .into_iter()
                .map(|item| ((query.sortKey)(&item), item))
                .collect();
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            let items: Vec<T> = keyed.into_iter().map(|(_, item)| item).collect();

            if results.wrappedValue() != items {
                results.set(items);
                true
            } else {
                false
            }
        });
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "query",
                detail: "(searchText: …, results: $…)",
                effect,
            },
        }
    }

    // MARK: - Paridade inert

    /// `.navigationDestination(for:) { … }` — inert (o NavigationPath fake
    /// carrega só descrições; o destino não monta).
    fn navigation_destination(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationDestination,
        }
    }

    /// `.navigationViewStyle(.stack)`
    fn navigation_view_style(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationViewStyle,
        }
    }

    /// `.attachEnvironmentOverrides()`
    fn attach_environment_overrides(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AttachEnvironmentOverrides,
        }
    }

    /// `.attachEnvironmentOverrides(onChange: …)`
    fn attach_environment_overrides_on_change(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AttachEnvironmentOverridesOnChange,
        }
    }

    /// `.flipsForRightToLeftLayoutDirection(true)`
    fn flips_for_right_to_left_layout_direction(self, flips: bool) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FlipsForRightToLeftLayoutDirection(flips),
        }
    }
}

impl<V: View<Arity = Single>> ViewExt for V {}
