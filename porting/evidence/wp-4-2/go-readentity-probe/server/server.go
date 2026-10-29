// Copies of server/rest.go's Item and Feedback.
package server

import "probe/data"

type Item struct {
	ItemId     string
	IsHidden   bool
	Categories []string
	Timestamp  string
	Labels     any
	Comment    string
}

type Feedback struct {
	data.FeedbackKey
	Value     float64
	Timestamp string
	Labels    any
	Comment   string
}
