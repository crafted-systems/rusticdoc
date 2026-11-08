# Office check

`templates/` holds templates typed by hand in Word, Excel and PowerPoint 16.114 for Mac. They have what real authoring leaves behind: curly quotes from AutoCorrect, a tag split by bold formatting, spelling marks, placeholders that inherit their position from the layout, and a slide loop that opens in the title and closes in the body. The same files are fixtures for `crates/template/tests/office.rs`.

`make office` (or `test/office/render.sh IMAGE`) fills them with `data.json`, once in their own format and once as PDF, into `out/<arch>/`. Then open each `.docx`, `.xlsx` and `.pptx` in its Office app:

- A repair prompt ("We found a problem with some content…") is a failure. After one, Excel shows "Repaired" in the title.
- Check the deck: one slide per product, and the chart and logo under their titles rather than in the corner.

Last run on 7 November 2026, Office 16.114.4 for Mac on macOS 27: no repair prompt in Word, Excel or PowerPoint. The run found three faults, all fixed:

1. Excel repaired every filled workbook, because its calculation chain still named the formula cells the loop had moved.
2. A loop opened in one shape and closed in another on the same slide printed the copies on top of each other.
3. Charts and pictures in a placeholder PowerPoint had not moved went to the slide's top-left corner.
