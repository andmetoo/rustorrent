# Rustorrent interface

A person checks transfers on a Mac laptop during the day, often alongside other work. Use a quiet light appearance, with a matching dark appearance for evening use. With no saved choice the interface follows the system appearance; the toolbar toggle and Settings › Appearance override it.

## Foundations
System font at 13 px; tabular numerals for transfer data. Restrained blue accent, subtly tinted neutral backgrounds, clear text, and fine 1 px separators. A compact type scale (11 px labels, 12 px secondary, 13 px body, 15 px titles) and predictable 4 px spacing. Status colors (blue active, green seeding, gray paused, red error) always come with a text label. Colors are CSS custom properties, redefined once for dark mode.

## Layout
A narrow sidebar holds library filters with counts (All, Downloading, Seeding, Paused, Completed, Errors), labels, and the Search, RSS and Settings tools, with a small rate history at the bottom. The toolbar shows the view title, current download and upload rates, a filter field and the Add button. The transfer list is the main workspace: one row per transfer with name, state pill, slim progress bar, rates, ETA, ratio, and quick actions (pause/resume, open folder, remove) that appear on hover, focus or selection. A row expands in place to tabs for Files, Trackers, Peers (with diagnostics) and Info (with label and less frequent actions). Below 720 px the sidebar becomes a horizontal filter strip and rows stack into two lines.

## Interaction
Use 150–200 ms transitions for feedback only, respect reduced motion, and show a strong focus outline. Dialogs have a visible title, inline validation, keyboard containment, Escape to cancel, and focus restoration. Destructive removal offers one explicit choice about downloaded files. Adding accepts a chosen or dropped .torrent file, a typed magnet, or a magnet pasted anywhere. Outcomes and failures appear as short toasts. Keyboard: `/` filter, `A` add, arrow keys move between transfers, Enter shows details, Space pauses or resumes, Delete removes.

## Live updates
The page renders from a JSON event stream that sends only what changed. Rows are keyed and updated in place, so focus, typed input, open details, selection and scroll position survive updates, and fixed column widths prevent layout shift.

## Empty and error states
The empty library explains how to add a file or magnet. Filtered lists distinguish no matches from an empty library. Connection interruptions are visible and recover automatically. Peer connection failures belong in diagnostics unless the entire torrent has failed.
