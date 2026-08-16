//
//  DetailRow.swift — CountriesSwiftUI
//
//  As sobrecargas `leftLabel:`/`rightLabel:` de Text e Image do Swift
//  colapsam numa struct genérica só — os labels são views tipadas.
//

use bunny_ui::prelude::*;

/// `DetailRow(leftLabel:rightLabel:)`
#[derive(Clone)]
pub struct DetailRow<L, R> {
    left_label: L,
    right_label: R,
}

impl<L: View<Arity = Single>, R: View<Arity = Single>> DetailRow<L, R> {
    pub fn new(left_label: L, right_label: R) -> Self {
        Self {
            left_label,
            right_label,
        }
    }
}

impl<L: View<Arity = Single>, R: View<Arity = Single>> Component for DetailRow<L, R> {
    fn body(self, _ctx: &Context) -> impl View {
        hstack((
            self.left_label.clone().font(Font::Headline),
            spacer(),
            self.right_label.clone().font(Font::Callout),
        ))
        .padding()
        .frame_max(f64::INFINITY, 40.0, Alignment::Leading)
    }
}
