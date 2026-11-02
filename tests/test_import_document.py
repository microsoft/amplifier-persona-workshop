import base64
import importlib.util
import io
from pathlib import Path
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('convert', Path(__file__).parents[1] / 'scripts/import-document.py')
convert = importlib.util.module_from_spec(spec)
spec.loader.exec_module(convert)


def doc(name, data, author='Avery'):
    return convert.convert({'filename': name, 'content_base64': base64.b64encode(data).decode(), 'author': author})


def archive(name, xml):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as z:
        z.writestr(name, xml)
    return out.getvalue()


class ImportTests(unittest.TestCase):
    def test_html_excludes_hidden_and_quoted_text(self):
        result = doc('a.html', b'<head>hidden</head><p>First thought.</p><blockquote>Peer writing</blockquote><script>secret</script><p>Second thought.</p>')
        text = ' '.join(m['text'] for m in result['messages'])
        self.assertIn('First thought.', text)
        self.assertIn('Second thought.', text)
        self.assertNotIn('hidden', text)
        self.assertNotIn('Peer', text)
        self.assertNotIn('secret', text)
        self.assertEqual(result['messages'][0]['author'], 'Avery')

    def test_docx_keeps_distinct_paragraphs_and_literal_labels(self):
        result = doc('a.docx', archive('word/document.xml', '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Plan: preserve this as authored text.</w:t></w:r></w:p><w:p><w:r><w:t>Next paragraph.</w:t></w:r></w:p></w:body></w:document>'))
        self.assertEqual(len(result['messages']), 2)
        self.assertEqual(result['messages'][0]['author'], 'Avery')
        self.assertEqual(result['messages'][0]['text'], 'Plan: preserve this as authored text.')

    def test_odt_and_unidentified_author(self):
        result = doc('a.odt', archive('content.xml', '<doc xmlns:t="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><t:p>A <t:span>connected</t:span> thought.</t:p></doc>'), '')
        self.assertEqual(result['messages'][0]['text'], 'A connected thought.')
        self.assertEqual(result['messages'][0]['author'], 'Unattributed')

    def test_email_decodes_mime_and_does_not_assign_reply_to_sender(self):
        result = doc('a.eml', b'From: Avery <avery@example.test>\r\nContent-Type: multipart/alternative; boundary="part"\r\n\r\n--part\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>A short idea.</p><blockquote>Different author</blockquote>\r\n--part--')
        self.assertEqual(result['kind'], 'email')
        self.assertEqual(result['messages'][0]['text'], 'A short idea.')
        self.assertEqual(result['messages'][0]['author'], 'Avery')

    def test_pdf_runs_installed_text_converter(self):
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
        result = doc('a.pdf', pdf)
        self.assertEqual(result['messages'][0]['text'], 'A concrete example.')


if __name__ == '__main__':
    unittest.main()
