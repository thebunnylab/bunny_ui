"""Build paragraph-context.ttf with fonttools==4.60.1; no runtime dependency.

One contextual substitution changes `a` only when 128 `b` glyphs and a final
`c` follow it. A truncated probe cannot see that distant dependency. All glyph
outlines and feature rules are created here; no external font is copied.
"""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.pens.ttGlyphPen import TTGlyphPen

order = ['.notdef', 'a', 'b', 'c', 'alternate', 'space']
font = FontBuilder(1000, isTTF=True)
font.setupGlyphOrder(order)
font.setupCharacterMap({ord('a'): 'a', ord('b'): 'b', ord('c'): 'c', 32: 'space'})
glyphs = {}
for index, name in enumerate(order):
    pen = TTGlyphPen(None)
    if name != 'space':
        right = 300 + index * 80
        pen.moveTo((50, 0)); pen.lineTo((right, 0))
        pen.lineTo((right, 700)); pen.lineTo((50, 700)); pen.closePath()
    glyphs[name] = pen.glyph()
font.setupGlyf(glyphs)
font.setupHorizontalMetrics({name: (600 if name != 'alternate' else 900, 50) for name in order})
font.setupHorizontalHeader(ascent=800, descent=-200)
font.setupNameTable({
    'familyName': 'Bunny Paragraph Context', 'styleName': 'Regular',
    'uniqueFontIdentifier': 'BunnyParagraphContext-Regular-1',
    'fullName': 'Bunny Paragraph Context Regular', 'psName': 'BunnyParagraphContext-Regular',
    'version': 'Version 1.000', 'copyright': 'Copyright 2026 Bunnylab. Test fixture.',
})
font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200, usMaxContext=130)
font.setupPost(); font.setupMaxp()
addOpenTypeFeaturesFromString(font.font, "languagesystem DFLT dflt; languagesystem latn dflt; feature calt { sub a' " + 'b ' * 128 + "c by alternate; } calt;")
font.font.recalcTimestamp = False
font.font['head'].created = font.font['head'].modified = 2082844800
font.save(Path(__file__).with_name('paragraph-context.ttf'))
