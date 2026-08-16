//! O modelo de edição de texto de UMA linha — headless por decisão.
//!
//! O app é dono da STRING (via `Binding<String>`); o framework é dono do
//! caret e da seleção. Os índices internos são offsets de BYTE sempre em
//! fronteira de `char`; a borda de IME fala UTF-16 — a conversão mora
//! aqui, UMA vez, em vez de copiada em cada campo artesanal.

/// Caret + âncora de seleção de um campo, por identidade. `caret` é o
/// ponto ativo; `anchor` marca o outro lado da seleção (None = sem
/// seleção); `marked` é a composição de IME viva (sublinhada, ainda não
/// committed). Offsets de byte; valores fora do texto clampam na
/// aplicação.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CaretState {
    pub caret: usize,
    pub anchor: Option<usize>,
    pub marked: Option<(usize, usize)>,
}

impl CaretState {
    /// A seleção normalizada `[início, fim)` — `None` = colapsada.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.caret {
            return None;
        }
        Some((anchor.min(self.caret), anchor.max(self.caret)))
    }
}

/// O que teclado e IME pedem a um campo. `bool` = estender a seleção
/// (shift pressionado). `Read`/`Copy`/`Cut` devolvem texto pela saída de
/// [`apply`] — a ponte de clipboard e de sincronização com a plataforma.
#[derive(Clone, PartialEq, Debug)]
pub enum EditCommand {
    Insert(String),
    Backspace,
    Delete,
    Left(bool),
    Right(bool),
    Home(bool),
    End(bool),
    SelectAll,
    /// Lê o texto inteiro sem mutar (a sincronização de IME usa).
    Read,
    /// Devolve a seleção sem mutar (`None` = colapsada).
    Copy,
    /// Devolve a seleção e a remove.
    Cut,
    /// Composição de IME: substitui o range marcado (ou a seleção) pelo
    /// texto em composição e o mantém MARCADO (sublinhado, não
    /// committed). `caret_utf16` = (location, length) DENTRO do texto
    /// marcado — o vocabulário da plataforma.
    SetMarked { text: String, caret_utf16: (usize, usize) },
    /// Encerra a composição committando o texto marcado como está.
    Unmark,
}

/// Fronteira de char anterior (ou 0).
fn previous_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    loop {
        if index == 0 {
            return 0;
        }
        index -= 1;
        if text.is_char_boundary(index) {
            return index;
        }
    }
}

/// Fronteira de char seguinte (ou o fim).
fn next_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    loop {
        if index >= text.len() {
            return text.len();
        }
        index += 1;
        if text.is_char_boundary(index) {
            return index;
        }
    }
}

fn clamp_to_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    if text.is_char_boundary(index) {
        index
    } else {
        previous_boundary(text, index)
    }
}

/// Clamp de um índice retido contra o texto corrente — a estampa usa.
pub(crate) fn clamp_index(text: &str, index: usize) -> usize {
    clamp_to_boundary(text, index)
}

/// Aplica um comando ao par (texto, caret) — a ÚNICA porta de mutação.
/// Estado fora do texto (o app trocou a string por fora) clampa aqui.
/// A saída é o texto que `Read`/`Copy`/`Cut` extraem.
pub fn apply(text: &mut String, state: &mut CaretState, command: EditCommand) -> Option<String> {
    state.caret = clamp_to_boundary(text, state.caret);
    if let Some(anchor) = state.anchor {
        state.anchor = Some(clamp_to_boundary(text, anchor));
    }
    if let Some((start, end)) = state.marked {
        let start = clamp_to_boundary(text, start);
        let end = clamp_to_boundary(text, end);
        state.marked = (start < end).then_some((start, end));
    }

    // qualquer comando que não seja de composição nem de leitura encerra
    // a composição viva (committa como está) antes de agir — exceto
    // Insert, que é o COMMIT da composição
    if state.marked.is_some()
        && !matches!(
            command,
            EditCommand::Read
                | EditCommand::Copy
                | EditCommand::Insert(_)
                | EditCommand::SetMarked { .. }
        )
    {
        state.marked = None;
    }

    // edição sobre seleção remove a seleção primeiro
    let remove_selection = |text: &mut String, state: &mut CaretState| {
        if let Some((start, end)) = state.selection() {
            text.replace_range(start..end, "");
            state.caret = start;
            state.anchor = None;
            true
        } else {
            state.anchor = None;
            false
        }
    };

    // movimento: com shift, a âncora arma no ponto atual e fica; sem
    // shift, uma seleção viva colapsa para a borda do movimento
    let moved = |state: &mut CaretState, select: bool, target: usize, collapse: usize| {
        if select {
            if state.anchor.is_none() {
                state.anchor = Some(state.caret);
            }
            state.caret = target;
        } else if state.selection().is_some() {
            state.caret = collapse;
            state.anchor = None;
        } else {
            state.caret = target;
            state.anchor = None;
        }
    };

    match command {
        EditCommand::Insert(insertion) => {
            // o commit da composição: o texto final substitui o marcado
            if let Some((start, end)) = state.marked.take() {
                text.replace_range(start..end, &insertion);
                state.caret = start + insertion.len();
                state.anchor = None;
            } else {
                remove_selection(text, state);
                text.insert_str(state.caret, &insertion);
                state.caret += insertion.len();
            }
        }
        EditCommand::SetMarked { text: composition, caret_utf16 } => {
            let (start, end) = state
                .marked
                .or(state.selection())
                .unwrap_or((state.caret, state.caret));
            text.replace_range(start..end, &composition);
            state.anchor = None;
            if composition.is_empty() {
                // composição esvaziada = cancelada
                state.marked = None;
                state.caret = start;
            } else {
                state.marked = Some((start, start + composition.len()));
                let (location, length) = caret_utf16;
                state.caret = start + utf16_to_byte(&composition, location + length);
            }
        }
        EditCommand::Unmark => {
            // o clamp/commit lá em cima já limpou; nada mais a fazer
        }
        EditCommand::Read => return Some(text.clone()),
        EditCommand::Copy => {
            return state.selection().map(|(start, end)| text[start..end].to_string());
        }
        EditCommand::Cut => {
            let cut = state.selection().map(|(start, end)| text[start..end].to_string());
            if cut.is_some() {
                remove_selection(text, state);
            }
            return cut;
        }
        EditCommand::Backspace => {
            if !remove_selection(text, state) && state.caret > 0 {
                let start = previous_boundary(text, state.caret);
                text.replace_range(start..state.caret, "");
                state.caret = start;
            }
        }
        EditCommand::Delete => {
            if !remove_selection(text, state) && state.caret < text.len() {
                let end = next_boundary(text, state.caret);
                text.replace_range(state.caret..end, "");
            }
        }
        EditCommand::Left(select) => {
            let target = previous_boundary(text, state.caret);
            let collapse = state.selection().map(|(start, _)| start).unwrap_or(target);
            moved(state, select, target, collapse);
        }
        EditCommand::Right(select) => {
            let target = next_boundary(text, state.caret);
            let collapse = state.selection().map(|(_, end)| end).unwrap_or(target);
            moved(state, select, target, collapse);
        }
        EditCommand::Home(select) => moved(state, select, 0, 0),
        EditCommand::End(select) => {
            let end = text.len();
            moved(state, select, end, end);
        }
        EditCommand::SelectAll => {
            state.anchor = Some(0);
            state.caret = text.len();
        }
    }
    None
}

// MARK: - A borda UTF-16 (IME e plataformas falam UTF-16; nós, bytes)

pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut count = 0;
    for (byte_index, ch) in text.char_indices() {
        if count >= utf16 {
            return byte_index;
        }
        count += ch.len_utf16();
    }
    text.len()
}

pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..clamp_to_boundary(text, byte)].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(caret: usize) -> CaretState {
        CaretState { caret, anchor: None, marked: None }
    }

    #[test]
    fn insert_backspace_and_delete_respect_char_boundaries() {
        let mut text = String::from("caé");
        let mut caret = state(text.len());

        apply(&mut text, &mut caret, EditCommand::Insert("!".into()));
        assert_eq!(text, "caé!");

        apply(&mut text, &mut caret, EditCommand::Backspace);
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "ca", "o é (2 bytes) sai inteiro");
        assert_eq!(caret.caret, 2);

        let mut caret = state(0);
        apply(&mut text, &mut caret, EditCommand::Delete);
        assert_eq!(text, "a");
        assert_eq!(caret.caret, 0);
    }

    #[test]
    fn selection_replaces_collapses_and_extends() {
        let mut text = String::from("hello world");
        let mut caret = state(0);

        apply(&mut text, &mut caret, EditCommand::SelectAll);
        assert_eq!(caret.selection(), Some((0, 11)));

        apply(&mut text, &mut caret, EditCommand::Insert("hi".into()));
        assert_eq!(text, "hi", "digitar sobre a seleção substitui");
        assert_eq!(caret.caret, 2);

        // shift+left arma a âncora e estende
        apply(&mut text, &mut caret, EditCommand::Left(true));
        assert_eq!(caret.selection(), Some((1, 2)));
        // left sem shift colapsa para a borda esquerda
        apply(&mut text, &mut caret, EditCommand::Left(false));
        assert_eq!(caret.caret, 1);
        assert_eq!(caret.selection(), None);

        apply(&mut text, &mut caret, EditCommand::End(true));
        assert_eq!(caret.selection(), Some((1, 2)));
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "h", "backspace come a seleção");
    }

    #[test]
    fn stale_caret_from_an_outside_write_clamps() {
        // o app trocou a string por fora — o estado retido clampa em vez
        // de estourar
        let mut text = String::from("ab");
        let mut caret = state(usize::MAX);
        apply(&mut text, &mut caret, EditCommand::Insert("c".into()));
        assert_eq!(text, "abc");
        assert_eq!(caret.caret, 3);
    }

    #[test]
    fn ime_composition_marks_replaces_and_commits() {
        let mut text = String::from("ab");
        let mut state = state(1); // entre a e b

        // composição incremental: cada SetMarked substitui o marcado
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "ｎ".into(), caret_utf16: (1, 0) },
        );
        assert_eq!(text, "aｎb");
        assert_eq!(state.marked, Some((1, 1 + "ｎ".len())));
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "に".into(), caret_utf16: (1, 0) },
        );
        assert_eq!(text, "aにb");
        assert_eq!(state.marked, Some((1, 1 + "に".len())));

        // o commit: Insert troca o marcado pelo texto final
        apply(&mut text, &mut state, EditCommand::Insert("日本".into()));
        assert_eq!(text, "a日本b");
        assert_eq!(state.marked, None);
        assert_eq!(state.caret, 1 + "日本".len());

        // composição esvaziada = cancelada
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "x".into(), caret_utf16: (1, 0) },
        );
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: String::new(), caret_utf16: (0, 0) },
        );
        assert_eq!(text, "a日本b");
        assert_eq!(state.marked, None);

        // movimento no meio da composição committa como está
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "y".into(), caret_utf16: (1, 0) },
        );
        apply(&mut text, &mut state, EditCommand::Left(false));
        assert_eq!(state.marked, None);
        assert!(text.contains('y'), "committado como estava: {text}");
    }

    #[test]
    fn the_utf16_border_translates_both_ways() {
        let text = "a🐰b"; // 🐰 = 4 bytes, 2 unidades UTF-16
        assert_eq!(byte_to_utf16(text, 0), 0);
        assert_eq!(byte_to_utf16(text, 1), 1);
        assert_eq!(byte_to_utf16(text, 5), 3, "depois do coelho: 1 + 2");
        assert_eq!(utf16_to_byte(text, 1), 1);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(utf16_to_byte(text, 99), text.len());
        // índice no MEIO do par substituto arredonda para a fronteira
        // SEGUINTE — nunca parte um char
        assert_eq!(utf16_to_byte(text, 2), 5, "meio do surrogate cai na fronteira seguinte");
    }
}
