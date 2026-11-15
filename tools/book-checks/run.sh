#!/bin/bash
# Checks how the iPhone and Mac apps read books in, against a shelf of real
# public-domain books: chapter lists, and every supported file format.
#
#   tools/book-checks/run.sh            run every check
#   tools/book-checks/run.sh mine       also list the chapters of each book
#                                       in tools/book-checks/private/
#
# The books are downloaded once into .cache/ (about 30 MB, not committed).
# The tools are compiled from the app's own import code, so this checks
# exactly what the app does. Needs a Mac with Xcode. Exits 1 if any check
# falls short of its bar.
set -uo pipefail
cd "$(dirname "$0")"

app=../../apple/ArcVoice
sources=(
    "$app/Services/BookFile.swift"
    "$app/Services/EPUBTextExtractor.swift"
    "$app/Services/TextCleaner.swift"
    "$app/Services/PDFTextExtractor.swift"
    "$app/Models/Chapter.swift"
)
cache=.cache
mkdir -p "$cache/books" "$cache/pdfs" "$cache/formats" "$cache/bin"

echo "Building the check tools from the app's code"
for tool in CheckEPUBs CheckPDFs CheckFormats ListChapters TypesetPDF; do
    swiftc -O -parse-as-library "$tool.swift" "${sources[@]}" -o "$cache/bin/$tool" || exit 1
done

echo "Fetching the shelf (first run only)"
# Project Gutenberg in two packagings (EPUB 3 with several chapters per
# file, EPUB 2 with toc.ncx) and Standard Ebooks (EPUB 3, a file per chapter).
gutenberg=(84:frankenstein 345:dracula 64317:gatsby 1661:sherlock 11:alice 2701:mobydick 1260:janeeyre 174:doriangray 1342:pride 120:treasure)
for book in "${gutenberg[@]}"; do
    id=${book%%:*}; name=${book##*:}
    [ -s "$cache/books/gut2-$name.epub" ] || curl -sfL -o "$cache/books/gut2-$name.epub" "https://www.gutenberg.org/ebooks/$id.epub.noimages"
done
for book in "${gutenberg[@]:0:8}"; do
    id=${book%%:*}; name=${book##*:}
    [ -s "$cache/books/gut3-$name.epub" ] || curl -sfL -o "$cache/books/gut3-$name.epub" "https://www.gutenberg.org/ebooks/$id.epub3.images"
done
standard=(jane-austen/pride-and-prejudice mary-shelley/frankenstein bram-stoker/dracula f-scott-fitzgerald/the-great-gatsby
          arthur-conan-doyle/the-adventures-of-sherlock-holmes robert-louis-stevenson/treasure-island charlotte-bronte/jane-eyre)
for book in "${standard[@]}"; do
    file=$(echo "$book" | tr '/' '_')
    [ -s "$cache/books/se-${book##*/}.epub" ] || curl -sfL -A "Mozilla/5.0" \
        -o "$cache/books/se-${book##*/}.epub" "https://standardebooks.org/ebooks/$book/downloads/$file.epub?source=download"
done
[ -s "$cache/formats/book.mobi" ] || curl -sfL -o "$cache/formats/book.mobi" "https://www.gutenberg.org/ebooks/1342.kindle.images"
[ -s "$cache/formats/book.azw3" ] || curl -sfL -o "$cache/formats/book.azw3" "https://www.gutenberg.org/ebooks/1342.kf8.images"

echo "Typesetting test PDFs in seven chapter-heading styles"
# Like ARCs: running headers, page numbers, chapters on fresh pages, no bookmarks.
styles=(chapter-title number-title-pov caps-words roman title-only number-only number-pov)
pdfbooks=(se-treasure-island se-pride-and-prejudice se-the-adventures-of-sherlock-holmes gut3-dracula se-jane-eyre
          se-the-great-gatsby gut3-doriangray gut3-alice gut3-frankenstein gut3-mobydick se-dracula se-frankenstein
          gut3-janeeyre gut2-treasure)
index=0
for book in "${pdfbooks[@]}"; do
    style=${styles[$((index % ${#styles[@]}))]}
    out="$cache/pdfs/$book--$style.pdf"
    [ -s "$out" ] || "$cache/bin/TypesetPDF" "$cache/books/$book.epub" "$style" "$out" 2>/dev/null
    index=$((index + 1))
done

echo "Making one file of every other format"
sample="$cache/formats"
if [ ! -s "$sample/sample.docx" ]; then
    {
        printf 'It is a truth universally acknowledged, that a single man in possession of a good fortune must be in want of a wife.\n\n'
        printf 'However little known the feelings or views of such a man may be on his first entering a neighbourhood, this truth is so well fixed in the minds of the surrounding families, that he is considered the rightful property of some one or other of their daughters.\n\n'
        printf '"My dear Mr. Bennet," said his lady to him one day, "have you heard that Netherfield Park is let at last?"\n\n'
        printf 'Mr. Bennet replied that he had not.\n\n"But it is," returned she; "for Mrs. Long has just been here, and she told me all about it."\n'
    } > "$sample/sample.txt"
    for kind in docx odt rtf html webarchive; do textutil -convert $kind "$sample/sample.txt" -output "$sample/sample.$kind"; done
    { echo "# Chapter One"; echo; cat "$sample/sample.txt"; } > "$sample/sample.md"
    printf 'not a book at all\x00\x01\x02' > "$sample/bad-junk.bin"
    printf '<fulfillmentToken xmlns="http://ns.adobe.com/adept">' > "$sample/bad-ticket.acsm"
fi

status=0
echo
echo "== EPUB chapters (from each book's own table of contents)"
"$cache/bin/CheckEPUBs" "$cache"/books/*.epub || status=1
echo
echo "== PDF chapters (no bookmarks: from type sizes and wording)"
"$cache/bin/CheckPDFs" "$cache"/pdfs/*.pdf 2>/dev/null || status=1
echo
echo "== Every format"
"$cache/bin/CheckFormats" "$sample"/sample.* "$sample"/book.* "$sample"/bad-* 2>/dev/null | grep -v "^    " || status=1

if [ "${1:-}" = "mine" ] && compgen -G "private/*" > /dev/null; then
    echo
    echo "== Your own books (private/, never committed)"
    "$cache/bin/ListChapters" private/* 2>/dev/null
fi

echo
[ $status -eq 0 ] && echo "All checks passed." || echo "Some checks fell short. See above."
exit $status
