//! O tema — tokens semânticos resolvidos em RUNTIME.
//!
//! O vocabulário é uma struct plana de tokens nomeados (descobrível,
//! tipada) com um acessor global por token: `theme::accent()` no call
//! site, sem contexto nenhum — é assim que centenas de leituras por
//! frame ficam ergonômicas. O store é um `Cell` thread-local (o mundo é
//! single-thread por design): trocar o tema é um `install(...)` — a
//! VERSÃO global bumpa e o próximo pass reconstrói a retenção (tokens
//! lidos em body ficam gravados na cena retida; a reconstrução é o preço
//! de UMA vez por troca, não por frame).
//!
//! Regra de leitura: chrome BUILT-IN (Button, Field, scrollbar) lê o
//! token na COLOCAÇÃO (place) — retheme repinta sem re-rodar body; app
//! lê onde quiser (`.foreground_color(theme::accent())` no body é o
//! comum, e a versão cobre a invalidação).

use std::cell::Cell;

use crate::layout::Color;

macro_rules! theme_tokens {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        /// O conjunto de tokens de um tema — plano, `Copy`, aberto a
        /// crescer junto com o chrome.
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct Theme {
            $($(#[$doc])* pub $name: Color,)+
        }

        $(
            $(#[$doc])*
            pub fn $name() -> Color {
                THEME.with(|theme| theme.get().$name)
            }
        )+
    };
}

theme_tokens! {
    /// O chão da janela.
    canvas,
    /// Superfície elevada (painéis, cards).
    panel,
    /// Texto principal.
    fg,
    /// Texto secundário (metadados, caminhos).
    fg_secondary,
    /// Texto apagado (badges, estados vazios).
    fg_faint,
    /// Texto de placeholder de campo.
    placeholder,
    /// A cor da marca — foco, links, highlights de match.
    accent,
    /// Borda de superfície.
    border,
    /// Linha divisória interna.
    divider,
    /// Fundo de row sob o ponteiro.
    row_hover,
    /// Fundo de row pressionada/ativa.
    row_pressed,
    /// Véu de seleção de texto.
    selection,
    /// Borda de campo focado.
    focus,
    /// O caret.
    caret,
    /// Fundo de controle (botão).
    control,
    /// Fundo de controle sob hover.
    control_hovered,
    /// Fundo de controle pressionado.
    control_pressed,
    /// Poço de campo de texto.
    field,
    /// Borda de campo em repouso.
    field_border,
    /// A thumb da scrollbar.
    scrollbar,
    /// O véu atrás de overlays.
    backdrop,
}

impl Theme {
    /// O tema-de-um-lápis claro — os valores que o framework sempre usou
    /// (os defaults dos testes dependem desta igualdade).
    pub const fn light() -> Theme {
        Theme {
            canvas: Color::hex(0xF2F3F7),
            panel: Color::WHITE,
            // o BLACK exato da casa: a tinta default de texto É este token
            // (os goldens headless contam com a igualdade)
            fg: Color::BLACK,
            fg_secondary: Color::hex(0x8A94A6),
            fg_faint: Color::hex(0xB3BAC7),
            placeholder: Color::hex(0x9AA2B1),
            accent: Color::hex(0x3B82F6),
            border: Color::hex_a(0x64748B55),
            divider: Color::hex_a(0x64748B2E),
            row_hover: Color::hex_a(0x3B82F617),
            row_pressed: Color::hex_a(0x3B82F62E),
            selection: Color::hex_a(0x3B82F640),
            focus: Color::hex(0x3B82F6),
            caret: Color::BLACK,
            control: Color::hex(0xDDE1E9),
            control_hovered: Color::hex(0xE7EAF1),
            control_pressed: Color::hex(0xC7CCD8),
            field: Color::WHITE,
            field_border: Color::OUTLINE,
            scrollbar: Color::rgba(0, 0, 0, 90),
            backdrop: Color::hex_a(0x0F172A55),
        }
    }

    /// O lado escuro do mesmo lápis.
    pub const fn dark() -> Theme {
        Theme {
            canvas: Color::hex(0x101014),
            panel: Color::hex(0x18181D),
            fg: Color::hex(0xEDEDF2),
            fg_secondary: Color::hex(0x9B9BA8),
            fg_faint: Color::hex(0x55555F),
            placeholder: Color::hex(0x6A6A76),
            accent: Color::hex(0x4C8DFF),
            border: Color::hex_a(0xFFFFFF14),
            divider: Color::hex_a(0xFFFFFF0D),
            row_hover: Color::hex_a(0x4C8DFF17),
            row_pressed: Color::hex_a(0x4C8DFF2E),
            selection: Color::hex_a(0x4C8DFF40),
            focus: Color::hex(0x4C8DFF),
            caret: Color::hex(0xEDEDF2),
            control: Color::hex(0x26262E),
            control_hovered: Color::hex(0x2F2F38),
            control_pressed: Color::hex(0x1E1E24),
            field: Color::hex(0x1E1E24),
            field_border: Color::hex_a(0xFFFFFF1A),
            scrollbar: Color::hex_a(0xFFFFFF2E),
            backdrop: Color::hex_a(0x00000066),
        }
    }
}

thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(Theme::light()) };
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// Troca o tema INTEIRO entre frames. A versão global bumpa — o próximo
/// pass de qualquer `Runtime` reconstrói a retenção uma vez.
pub fn install(theme: Theme) {
    THEME.with(|current| current.set(theme));
    VERSION.with(|version| version.set(version.get() + 1));
}

/// O snapshot corrente — para hot loops que leem muitos tokens.
pub fn current() -> Theme {
    THEME.with(|theme| theme.get())
}

/// A versão do tema instalado — quem retém saída derivada compara.
pub fn version() -> u64 {
    VERSION.with(|version| version.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_swaps_tokens_and_bumps_the_version() {
        let before = version();
        install(Theme::dark());
        assert_eq!(accent(), Theme::dark().accent);
        assert_eq!(version(), before + 1);
        install(Theme::light());
        assert_eq!(accent(), Theme::light().accent);
        assert_eq!(version(), before + 2);
    }
}
