// Copies of storage/data/database.go's types, with their JSON-relevant fields.
package data

import "time"

type User struct {
	UserId   string    `gorm:"primaryKey" mapstructure:"user_id"`
	Labels   any       `gorm:"serializer:json" mapstructure:"labels"`
	Comment  string    `mapstructure:"comment"`
	UpdateAt time.Time `gorm:"column:update_at" mapstructure:"-" json:"-"`
}

type UserPatch struct {
	Labels  any
	Comment *string
}

type ItemPatch struct {
	IsHidden   *bool
	Categories []string
	Timestamp  *time.Time
	Labels     any
	Comment    *string
}

type FeedbackKey struct {
	FeedbackType string `gorm:"column:feedback_type" mapstructure:"feedback_type"`
	UserId       string `gorm:"column:user_id" mapstructure:"user_id"`
	ItemId       string `gorm:"column:item_id" mapstructure:"item_id"`
}
