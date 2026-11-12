"""Bundled pypdf qualification, run by the exact private Python in native CI."""
import base64, importlib.util, io
from pathlib import Path
import unittest
from pypdf import PdfReader, PdfWriter
spec=importlib.util.spec_from_file_location('converter',Path(__file__).parents[1]/'scripts/import-document.py')
converter=importlib.util.module_from_spec(spec);spec.loader.exec_module(converter)
def text_pdf():
    parts = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>', b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>', b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    stream = b'BT /F1 12 Tf 20 250 Td (A concrete example.) Tj ET'
    parts.append(b'<< /Length ' + str(len(stream)).encode() + b' >>\nstream\n' + stream + b'\nendstream')
    pdf = b'%PDF-1.4\n'; offsets = [0]
    for i, part in enumerate(parts, 1):
        offsets.append(len(pdf)); pdf += f'{i} 0 obj\n'.encode() + part + b'\nendobj\n'
    pos = len(pdf)
    pdf += f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
    pdf += b''.join(f'{o:010d} 00000 n \n'.encode() for o in offsets[1:])
    pdf += f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{pos}\n%%EOF'.encode()
    return pdf

def convert(data):
    return converter.convert({'filename':'fixture.pdf','pdf_backend':'pypdf','content_base64':base64.b64encode(data).decode(),'author':'Avery'})

class NativePDF(unittest.TestCase):
    def test_text_and_literal_attribution(self):
        result=convert(text_pdf())
        self.assertEqual(result['messages'][0]['text'],'A concrete example.')
        self.assertEqual(result['messages'][0]['author'],'Avery')
    def test_encrypted_never_asks_for_or_guesses_password(self):
        writer=PdfWriter();writer.append(PdfReader(io.BytesIO(text_pdf())));writer.encrypt('private',algorithm='RC4-128');data=io.BytesIO();writer.write(data)
        with self.assertRaisesRegex(ValueError,'Encrypted'):convert(data.getvalue())
    def test_scanned_or_empty_and_corrupt_rejected(self):
        writer=PdfWriter();writer.add_blank_page(width=100,height=100);data=io.BytesIO();writer.write(data)
        with self.assertRaisesRegex(ValueError,'No readable text'):convert(data.getvalue())
        with self.assertRaises(Exception):convert(b'not a PDF')
    def test_compressed_stream_expansion_is_bounded(self):
        import zlib
        from pypdf import apply_configuration
        from pypdf.filters import decompress
        bomb=zlib.compress(b'a'*(converter.LIMIT*17))
        with apply_configuration(zlib_maximum_output_length=16*converter.LIMIT):
            with self.assertRaises(Exception):decompress(bomb)

if __name__=='__main__':unittest.main()
