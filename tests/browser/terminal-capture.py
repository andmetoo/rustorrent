#!/usr/bin/env python3
"""Runs a terminal program in a pseudo-terminal and prints its final screen as HTML.

Used by screenshots.spec.mjs to picture the terminal interface and CLI. Only the
escape sequences rustorrent emits are interpreted: cursor position, erase, and SGR.

usage: terminal-capture.py ROWS COLS KEYS -- command [args...]
KEYS is a '|'-separated list of key strings (Python escapes allowed), each sent
after the screen has settled.
"""
import html
import os
import pty
import re
import select
import struct
import sys
import termios
import fcntl
import time

rows, cols, keys = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
command = sys.argv[sys.argv.index('--') + 1:]

pid, fd = pty.fork()
if pid == 0:
    os.environ['TERM'] = 'xterm-256color'
    os.execvp(command[0], command)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))

raw = bytearray()


def pump(seconds):
    end = time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return False
            if not data:
                return False
            raw.extend(data)
    return True


alive = pump(2.5)
for key in filter(None, keys.split('|')):
    if not alive:
        break
    os.write(fd, key.encode().decode('unicode_escape').encode('latin-1').decode('utf-8').encode())
    alive = pump(1.4)
if alive:
    pump(1.0)

# --- a tiny VT interpreter ---------------------------------------------------
blank = (' ', ())
screen = [[blank] * cols for _ in range(rows)]
row = col = 0
style = {}
text = raw.decode('utf-8', 'replace')
alt_start = text.rfind('\x1b[?1049h')
if alt_start >= 0:
    text = text[alt_start:]
end_alt = text.find('\x1b[?1049l')
if end_alt >= 0:
    text = text[:end_alt]

token = re.compile(r'\x1b\[([?0-9;]*)([A-Za-z])|\x1b.|\r|\n|[^\x1b\r\n]')
for m in token.finditer(text):
    if m.group(2):
        params, final = m.group(1), m.group(2)
        if params.startswith('?'):
            continue
        nums = [int(p) if p else 0 for p in params.split(';')] if params else []
        if final == 'H':
            row = max(1, nums[0] if nums else 1) - 1
            col = max(1, nums[1] if len(nums) > 1 else 1) - 1
        elif final == 'J':
            screen = [[blank] * cols for _ in range(rows)]
        elif final == 'K':
            if row < rows:
                for c in range(col, cols):
                    screen[row][c] = blank
        elif final == 'm':
            for n in nums or [0]:
                if n == 0:
                    style = {}
                elif n in (1, 2, 4, 7):
                    style[n] = True
                elif n == 22:
                    style.pop(1, None)
                    style.pop(2, None)
                elif 30 <= n <= 37 or 90 <= n <= 97:
                    style['fg'] = n
                elif n == 39:
                    style.pop('fg', None)
        continue
    ch = m.group(0)
    if ch == '\r':
        col = 0
    elif ch == '\n':
        row += 1
        col = 0
        if row >= rows:
            screen.pop(0)
            screen.append([blank] * cols)
            row = rows - 1
    elif not ch.startswith('\x1b'):
        if row < rows and col < cols:
            screen[row][col] = (ch, tuple(sorted(style.items(), key=str)))
        col += 1

try:
    os.kill(pid, 15)
except OSError:
    pass

out = []
for line in screen:
    spans, current, buf = [], None, ''
    for ch, st in line:
        if st != current:
            if buf:
                spans.append((current, buf))
            current, buf = st, ''
        buf += ch
    spans.append((current, buf))
    parts = []
    for st, chunk in spans:
        st = dict(st or ())
        classes = [f'f{st["fg"]}'] if 'fg' in st else []
        classes += [name for code, name in ((1, 'b'), (2, 'd'), (4, 'u'), (7, 'r')) if st.get(code)]
        chunk = html.escape(chunk)
        parts.append(f'<span class="{" ".join(classes)}">{chunk}</span>' if classes else chunk)
    out.append(''.join(parts).rstrip())
print('\n'.join(out))
