package chat

import (
	"fmt"
	"html"
	"strconv"
	"strings"
)

var ansiSGR = rx(`\x1b\[([0-9;:]*)m|\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]`)

type sgr struct {
	fg, bg                   int
	fgRGB, bgRGB             string
	bold, dim, italic, under bool
	inverse                  bool
}

func newSGR() sgr { return sgr{fg: -1, bg: -1} }

func (s sgr) plain() bool { return s == newSGR() }

// hasAnsi reports whether s carries escape sequences.
func hasAnsi(s string) bool { return strings.ContainsRune(s, '\x1b') }

// AnsiHTML renders SGR colours as escaped HTML spans: the 16 colours as classes af0..af15 / ab0..ab15 (the theme
// picks them), 256 and truecolor as inline rgb, bold a-b, dim a-d, italic a-i, underline a-u. Every other
// escape is dropped. The input is an already laid-out screen, not a stream.
func AnsiHTML(text string) string {
	var sb strings.Builder
	st := newSGR()
	open := false
	pos := 0
	put := func(s string) {
		if s == "" {
			return
		}
		if !open && !st.plain() {
			sb.WriteString(openSpan(st))
			open = true
		}
		sb.WriteString(html.EscapeString(s))
	}
	for _, m := range ansiSGR.FindAllStringSubmatchIndex(text, -1) {
		put(text[pos:m[0]])
		pos = m[1]
		if m[2] < 0 { // not SGR: dropped
			continue
		}
		next := applySGR(st, text[m[2]:m[3]])
		if next == st {
			continue
		}
		if open {
			sb.WriteString("</span>")
			open = false
		}
		st = next
	}
	put(text[pos:])
	if open {
		sb.WriteString("</span>")
	}
	return sb.String()
}

func clamp(v int) int {
	if v < 0 {
		return 0
	}
	if v > 255 {
		return 255
	}
	return v
}

func applySGR(s sgr, codes string) sgr {
	var p []int
	if codes == "" {
		p = []int{0}
	} else {
		for _, x := range strings.FieldsFunc(codes, func(r rune) bool { return r == ';' || r == ':' }) {
			n, _ := strconv.Atoi(x)
			p = append(p, n)
		}
		if len(p) == 0 {
			p = []int{0}
		}
	}
	for i := 0; i < len(p); i++ {
		c := p[i]
		switch {
		case c == 0:
			s = newSGR()
		case c == 1:
			s.bold = true
		case c == 2:
			s.dim = true
		case c == 3:
			s.italic = true
		case c == 4:
			s.under = true
		case c == 7:
			s.inverse = true
		case c == 22:
			s.bold, s.dim = false, false
		case c == 23:
			s.italic = false
		case c == 24:
			s.under = false
		case c == 27:
			s.inverse = false
		case c >= 30 && c <= 37:
			s.fg, s.fgRGB = c-30, ""
		case c == 39:
			s.fg, s.fgRGB = -1, ""
		case c >= 40 && c <= 47:
			s.bg, s.bgRGB = c-40, ""
		case c == 49:
			s.bg, s.bgRGB = -1, ""
		case c >= 90 && c <= 97:
			s.fg, s.fgRGB = c-90+8, ""
		case c >= 100 && c <= 107:
			s.bg, s.bgRGB = c-100+8, ""
		case c == 38 || c == 48:
			rgb, idx := "", -1
			if i+2 < len(p) && p[i+1] == 5 {
				idx = p[i+2]
				i += 2
			} else if i+4 < len(p) && p[i+1] == 2 {
				rgb = fmt.Sprintf("rgb(%d,%d,%d)", clamp(p[i+2]), clamp(p[i+3]), clamp(p[i+4]))
				i += 4
			}
			if idx >= 0 && idx < 16 {
				if c == 38 {
					s.fg, s.fgRGB = idx, ""
				} else {
					s.bg, s.bgRGB = idx, ""
				}
			} else if idx >= 16 {
				rgb = xterm256(idx)
			}
			if rgb != "" {
				if c == 38 {
					s.fgRGB, s.fg = rgb, -1
				} else {
					s.bgRGB, s.bg = rgb, -1
				}
			}
		}
	}
	return s
}

func xterm256(n int) string {
	if n < 16 {
		n = 16
	}
	if n > 255 {
		n = 255
	}
	if n >= 232 {
		g := 8 + (n-232)*10
		return fmt.Sprintf("rgb(%d,%d,%d)", g, g, g)
	}
	n -= 16
	l := func(v int) int {
		if v == 0 {
			return 0
		}
		return 55 + v*40
	}
	return fmt.Sprintf("rgb(%d,%d,%d)", l(n/36), l(n/6%6), l(n%6))
}

func openSpan(s sgr) string {
	fg, bg, fgRGB, bgRGB := s.fg, s.bg, s.fgRGB, s.bgRGB
	if s.inverse {
		fg, bg, fgRGB, bgRGB = s.bg, s.fg, s.bgRGB, s.fgRGB
		if s.bg < 0 && s.bgRGB == "" {
			fg = 16
		}
		if s.fg < 0 && s.fgRGB == "" {
			bg = 17
		}
	}
	var cls, style []string
	if fgRGB != "" {
		style = append(style, "color:"+fgRGB)
	} else if fg >= 0 {
		cls = append(cls, "af"+strconv.Itoa(fg))
	}
	if bgRGB != "" {
		style = append(style, "background:"+bgRGB)
	} else if bg >= 0 {
		cls = append(cls, "ab"+strconv.Itoa(bg))
	}
	if s.bold {
		cls = append(cls, "a-b")
	}
	if s.dim {
		cls = append(cls, "a-d")
	}
	if s.italic {
		cls = append(cls, "a-i")
	}
	if s.under {
		cls = append(cls, "a-u")
	}
	out := "<span"
	if len(cls) > 0 {
		out += ` class="` + strings.Join(cls, " ") + `"`
	}
	if len(style) > 0 {
		out += ` style="` + strings.Join(style, ";") + `"`
	}
	return out + ">"
}
