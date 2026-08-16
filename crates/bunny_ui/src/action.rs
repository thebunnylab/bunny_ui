//! Ações nomeadas + padrões de tecla — o menor keymap com a forma certa.
//!
//! Tecla vira INTENÇÃO (`KeyPattern → ActionId`, no keymap do `Runtime`);
//! intenção acha o HANDLER vigente (`.on_action`, retido no reconciler
//! como as ações de clique — o mais interno vence). O shell só traduz e
//! compõe: match → dispatch → repaint. Binding sem handler montado não
//! consome a tecla — a tela sem a palette digita normal.

/// Identidade nominal de uma ação — declarada como const pelo app:
/// `const SELECT_NEXT: ActionId = ActionId("finder.select_next");`
/// Namespace por convenção (`"app.acao"`); a string imprime e serializa
/// (debug do mapa hoje, keymap configurável amanhã).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionId(pub &'static str);

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Teclas com nome + o caso imprimível (minúsculo, sem modificador).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Down,
    Up,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Home,
    End,
    Char(char),
}

/// O padrão que o keymap casa — modificadores EXATOS (Cmd+Enter não casa
/// o binding de Enter). `Eq + Hash`: chave direta do mapa.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyPattern {
    pub key: Key,
    pub shift: bool,
    pub command: bool,
    pub option: bool,
    pub control: bool,
}

impl KeyPattern {
    /// A tecla nua.
    pub const fn key(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: false, control: false }
    }

    /// Cmd + tecla.
    pub const fn command(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: true, option: false, control: false }
    }

    /// Shift + tecla.
    pub const fn shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: false, option: false, control: false }
    }

    /// Char nu (sem cmd/ctrl) é DIGITAÇÃO: com campo focado, o gate deixa
    /// passar para o texto sem consultar o mapa — bindado ou não. (Option
    /// conta como digitação: option+a compõe "å" no macOS.)
    pub fn is_text_input(&self) -> bool {
        matches!(self.key, Key::Char(_)) && !self.command && !self.control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_patterns_are_recognized() {
        assert!(KeyPattern::key(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::command(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::key(Key::Down).is_text_input());
        let option_a = KeyPattern { option: true, ..KeyPattern::key(Key::Char('a')) };
        assert!(option_a.is_text_input(), "option compõe texto no macOS");
    }
}
