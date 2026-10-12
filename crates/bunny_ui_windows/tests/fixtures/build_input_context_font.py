"""Build paragraph-input-context.ttf with fonttools==4.60.1.

The same 130 source characters as paragraph-context.ttf are now all marked as
lookup input, not one target plus lookahead. This distinguishes input-pattern
limits from source-context preservation. All outlines/rules are original.
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
    'familyName': 'Bunny Paragraph Input Context', 'styleName': 'Regular',
    'uniqueFontIdentifier': 'BunnyParagraphInputContext-Regular-1',
    'fullName': 'Bunny Paragraph Input Context Regular', 'psName': 'BunnyParagraphInputContext-Regular',
    'version': 'Version 1.000', 'copyright': 'Copyright 2026 Bunnylab. Test fixture.',
})
font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200, usMaxContext=130)
font.setupPost(); font.setupMaxp()
addOpenTypeFeaturesFromString(font.font, "languagesystem DFLT dflt; languagesystem latn dflt; lookup Replace { sub a by alternate; } Replace; feature calt { sub a' lookup Replace " + "b' " * 128 + "c'; } calt;")
font.font.recalcTimestamp = False
font.font['head'].created = font.font['head'].modified = 2082844800
font.save(Path(__file__).with_name('paragraph-input-context.ttf'))
