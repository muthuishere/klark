package chat_test

import (
	"fmt"
	"strings"

	"github.com/muthuishere/klark/hub-go/chat"
)

func Example() {
	screen := "> fix the test\n\n⏺ Fixed it in src/lib.rs:12.\n"
	rows := chat.ScreenItems(chat.Transform("claude", screen))
	for _, r := range rows {
		fmt.Println(r.Role, r.Kind, strings.TrimSpace(r.Text))
	}
	html := chat.Linkify(rows[len(rows)-1].HTML, func(path string, line int) string {
		return fmt.Sprintf("/open?path=%s&line=%d", path, line)
	})
	fmt.Println(strings.Contains(html, `href="/open?path=src/lib.rs&amp;line=12"`))
	// Output:
	// user text fix the test
	// assistant text Fixed it in src/lib.rs:12.
	// true
}

func ExampleChoicePrompt() {
	b := chat.ChoicePrompt("Do you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel")
	if b != nil {
		fmt.Println(b.Question, len(b.Choices), b.Selected)
	}
}
