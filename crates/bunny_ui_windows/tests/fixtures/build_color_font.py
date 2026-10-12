"""Rebuild paragraph-color.ttf with fonttools==4.60.1; no runtime dependency.

The five private test glyphs have deliberately distinct presentations: outline,
COLRv0 palette layers, SVG, COLRv1 gradient and a PNG bitmap in an sbix strike.
All shapes and bitmap pixels are constructed here; no external font is copied.
"""
from pathlib import Path
import struct
import zlib
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables.S_V_G_ import SVGDocument
from fontTools.ttLib.tables.sbixStrike import Strike
from fontTools.ttLib.tables.sbixGlyph import Glyph
from fontTools.ttLib.tables.otTables import PaintFormat, ExtendMode
from fontTools.colorLib.builder import buildCOLR, buildCPAL

order = ['.notdef', 'mono', 'colr', 'svg', 'gradient', 'bitmap', 'red', 'blue']
font = FontBuilder(1000, isTTF=True)
font.setupGlyphOrder(order)
font.setupCharacterMap({ord('a') + i: name for i, name in enumerate(order[1:6])})
def rectangle(left, bottom, right, top):
    pen = TTGlyphPen(None)
    pen.moveTo((left, bottom)); pen.lineTo((right, bottom))
    pen.lineTo((right, top)); pen.lineTo((left, top)); pen.closePath()
    return pen.glyph()
glyphs = {name: rectangle(100, 0, 900, 800) for name in order}
glyphs['red'] = rectangle(100, 0, 500, 800)
glyphs['blue'] = rectangle(500, 0, 900, 800)
font.setupGlyf(glyphs)
font.setupHorizontalMetrics({name: (1000, 100) for name in order})
font.setupHorizontalHeader(ascent=800, descent=-200)
font.setupNameTable({'familyName':'Bunny Paragraph Color', 'styleName':'Regular',
    'uniqueFontIdentifier':'BunnyParagraphColor-Regular-1',
    'fullName':'Bunny Paragraph Color Regular', 'psName':'BunnyParagraphColor-Regular',
    'version':'Version 1.000', 'copyright':'Copyright 2026 Bunnylab. Test fixture.'})
font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
font.setupPost(); font.setupMaxp()
font.font['CPAL'] = buildCPAL([[(1,0,0,1), (0,0,1,1)]])
font.font['COLR'] = buildCOLR({
    'colr': [('red',0),('blue',1)],
    'gradient': {'Format':PaintFormat.PaintGlyph, 'Glyph':'mono', 'Paint':{
        'Format':PaintFormat.PaintLinearGradient, 'x0':100, 'y0':0,
        'x1':900, 'y1':0, 'x2':100, 'y2':800,
        'ColorLine':{'Extend':ExtendMode.PAD, 'ColorStop':[
            {'StopOffset':0, 'PaletteIndex':0, 'Alpha':1},
            {'StopOffset':1, 'PaletteIndex':1, 'Alpha':1}]}}}
}, glyphMap=font.font.getReverseGlyphMap())
font.font['SVG '] = newTable('SVG ')
font.font['SVG '].docList = [SVGDocument(
    '<svg xmlns="http://www.w3.org/2000/svg"><g id="glyph3">'
    '<rect x="100" y="-800" width="400" height="800" fill="#00ff00"/>'
    '<rect x="500" y="-800" width="400" height="800" fill="#ff00ff"/>'
    '</g></svg>', 3, 3)]
def chunk(name, data):
    return struct.pack('>I',len(data))+name+data+struct.pack('>I',zlib.crc32(name+data)&0xffffffff)
rows = b''.join(b'\0'+b''.join(bytes((0,255,255,255) if x<16 else (255,255,0,255)) for x in range(32)) for _ in range(32))
png = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',32,32,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(rows))+chunk(b'IEND',b'')
strike = Strike(ppem=32, resolution=72)
strike.glyphs['bitmap'] = Glyph(glyphName='bitmap', originOffsetX=0, originOffsetY=0,
    graphicType='png ', imageData=png)
font.font['sbix'] = newTable('sbix'); font.font['sbix'].version=1; font.font['sbix'].flags=1
font.font['sbix'].strikes={32:strike}
font.font.recalcTimestamp=False
font.font['head'].created=font.font['head'].modified=2082844800
font.save(Path(__file__).with_name('paragraph-color.ttf'))
