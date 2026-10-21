package wa

import (
	"errors"
	"fmt"
)

// ErrKind names the type of err's innermost cause, for logs that must never print an error's text
// (it can carry a JID, a path or a URL). It follows Unwrap down to the last error that wraps
// nothing; of an error that wraps several (errors.Join, fmt's "%w: %w"), it follows the last one,
// since a sentinel comes first and the cause after it. An error that wraps nothing is named itself,
// so the answer is never "<nil>" for a non-nil err.
func ErrKind(err error) string {
	for err != nil {
		var next error
		switch e := err.(type) {
		case interface{ Unwrap() []error }:
			for _, c := range e.Unwrap() {
				if c != nil {
					next = c
				}
			}
		default:
			next = errors.Unwrap(err)
		}
		if next == nil {
			break
		}
		err = next
	}
	return fmt.Sprintf("%T", err)
}
