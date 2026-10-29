// Command redisfreshgolden runs Go's Redis UpdateScores on fresh RediSearch
// indexes (X-246, X-249). Each trial makes a new index, adds five subsets of one
// tied document, updates its categories with UpdateScores at
// WithMaxSearchResults(2), and then counts three things:
//
//   - hash: the trials in which a hash still lacks the new categories. This
//     reads each hash with HGET and does not use the index, so it is the lost
//     update itself.
//   - search: the trials in which FT.SEARCH, run at once, finds fewer than five
//     documents with the new categories. The index is updated in the background,
//     so this can be short while the update is in the hashes already.
//   - waited: the same search, after FT.INFO reports percent_indexed 1 and
//     indexing 0. A short count here is a lost update in the index.
//
// The tool is a diagnostic. It was written to find the Go side of the X-246 loss
// and found none in the conditions tried; PORTING_NOTES.md X-246 records the
// counts. Run it with REDIS_URI set:
//
//	REDIS_URI=redis://127.0.0.1:6379 go run ./contracts/tools/redisfreshgolden -trials 1000 -out result.json
//
// With -probe sorted it runs the X-249 probe instead: one SearchScores right after
// one AddScores on a fresh index.
package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"strings"
	"time"

	"github.com/gorse-io/gorse/storage"
	"github.com/gorse-io/gorse/storage/cache"
	"github.com/redis/go-redis/v9"
)

// listed is the most trial numbers a report lists for one count.
const listed = 50

// report is what the tool writes. A count is the number of trials that were
// short; the lists hold the first trial numbers of each count.
type report struct {
	Trials       int   `json:"trials"`
	HashLost     int   `json:"hash_lost"`
	HashMissed   int   `json:"hash_missed"`
	HashLostList []int `json:"hash_lost_trials"`
	Search       int   `json:"search_lost"`
	SearchList   []int `json:"search_lost_trials"`
	Waited       int   `json:"search_lost_waited"`
	WaitedList   []int `json:"search_lost_waited_trials"`
	WaitTimeouts int   `json:"wait_timeouts"`
}

func main() {
	trials := flag.Int("trials", 1000, "the number of fresh indexes")
	out := flag.String("out", "", "the file to write the report to (stdout when empty)")
	probe := flag.String("probe", "fresh", "fresh: UpdateScores on fresh indexes (X-246); sorted: SearchScores at once after AddScores (X-249)")
	noWait := flag.Bool("nowait", false, "skip the index wait, for a server with a standing backlog")
	flag.Parse()

	dsn := os.Getenv("REDIS_URI")
	if dsn == "" {
		log.Fatal("REDIS_URI is not set")
	}
	opt, err := redis.ParseURL(dsn)
	if err != nil {
		log.Fatal(err)
	}
	// The cache sets the same protocol for its own client (storage/cache/redis.go).
	opt.Protocol = 2
	client := redis.NewClient(opt)
	defer client.Close()

	ctx := context.Background()
	if *probe == "sorted" {
		writeReport(*out, sortedProbe(ctx, dsn, client, *trials, *noWait))
		return
	}
	run := time.Now().UnixNano()
	result := report{
		Trials:       *trials,
		HashLostList: []int{},
		SearchList:   []int{},
		WaitedList:   []int{},
	}
	for trial := 0; trial < *trials; trial++ {
		prefix := fmt.Sprintf("gorse_fresh_%d_%d_", run, trial)
		db, err := cache.Open(dsn, prefix, storage.WithMaxSearchResults(2))
		if err != nil {
			log.Fatal(err)
		}
		if err := db.Init(); err != nil {
			log.Fatal(err)
		}
		for i := 0; i < 5; i++ {
			subset := fmt.Sprintf("tie-subset-%d", i)
			doc := cache.Score{Id: "shared-item", Score: 1, Categories: []string{"tie-old"}, Timestamp: time.Now().UTC()}
			if err := db.AddScores(ctx, "collection-c", subset, []cache.Score{doc}); err != nil {
				log.Fatal(err)
			}
		}
		patch := cache.ScorePatch{Categories: []string{"tie-new"}}
		if err := db.UpdateScores(ctx, []string{"collection-c"}, nil, "shared-item", patch); err != nil {
			log.Fatal(err)
		}

		table := storage.TablePrefix(prefix).DocumentTable()
		search := func() int {
			query := fmt.Sprintf("@collection:{ %s } @categories:{ %s }", escape("collection-c"), escape(encodeCategory("tie-new")))
			found, err := client.FTSearchWithArgs(ctx, table, query, &redis.FTSearchOptions{NoContent: true}).Result()
			if err != nil {
				log.Fatal(err)
			}
			return found.Total
		}
		if search() != 5 {
			result.Search++
			result.SearchList = appendListed(result.SearchList, trial)
		}
		missed := 0
		for i := 0; i < 5; i++ {
			key := table + ":" + "collection-c" + ":" + fmt.Sprintf("tie-subset-%d", i) + ":" + "shared-item"
			v, err := client.HGet(ctx, key, "categories").Result()
			if err != nil {
				log.Fatal(err)
			}
			if v != encodeCategory("tie-new") {
				missed++
			}
		}
		if missed > 0 {
			result.HashLost++
			result.HashMissed += missed
			result.HashLostList = appendListed(result.HashLostList, trial)
		}
		if !*noWait {
			if !waitIndexed(ctx, client, table) {
				result.WaitTimeouts++
			} else if search() != 5 {
				result.Waited++
				result.WaitedList = appendListed(result.WaitedList, trial)
			}
		}

		if err := client.FTDropIndexWithArgs(ctx, table, &redis.FTDropIndexOptions{DeleteDocs: true}).Err(); err != nil {
			log.Fatal(err)
		}
		if err := db.Close(); err != nil {
			log.Fatal(err)
		}
	}

	writeReport(*out, result)
}

// sortedReport is the output of the sorted probe. A trial is short when the
// sorted SearchScores, run at once after AddScores, does not return the one
// document added, and short when waited when it still does not after the index
// has caught up.
type sortedReport struct {
	Trials       int   `json:"trials"`
	Short        int   `json:"search_short"`
	ShortList    []int `json:"search_short_trials"`
	Waited       int   `json:"search_short_waited"`
	WaitedList   []int `json:"search_short_waited_trials"`
	WaitTimeouts int   `json:"wait_timeouts"`
}

// sortedProbe runs one fresh index per trial: it adds one document, and searches
// for it with the sorted SearchScores that the escape test uses (X-249).
func sortedProbe(ctx context.Context, dsn string, client *redis.Client, trials int, noWait bool) sortedReport {
	report := sortedReport{Trials: trials, ShortList: []int{}, WaitedList: []int{}}
	run := time.Now().UnixNano()
	for trial := 0; trial < trials; trial++ {
		prefix := fmt.Sprintf("gorse_sorted_%d_%d_", run, trial)
		db, err := cache.Open(dsn, prefix, storage.WithMaxSearchResults(2))
		if err != nil {
			log.Fatal(err)
		}
		if err := db.Init(); err != nil {
			log.Fatal(err)
		}
		doc := cache.Score{Id: "doc", Score: 1, Categories: []string{"tie-old"}, Timestamp: time.Now().UTC()}
		if err := db.AddScores(ctx, "collection-c", "tie-subset-0", []cache.Score{doc}); err != nil {
			log.Fatal(err)
		}
		found := func() int {
			docs, err := db.SearchScores(ctx, "collection-c", "tie-subset-0", []string{"tie-old"}, 0, -1)
			if err != nil {
				log.Fatal(err)
			}
			return len(docs)
		}
		if found() != 1 {
			report.Short++
			report.ShortList = appendListed(report.ShortList, trial)
		}
		table := storage.TablePrefix(prefix).DocumentTable()
		if !noWait {
			if !waitIndexed(ctx, client, table) {
				report.WaitTimeouts++
			} else if found() != 1 {
				report.Waited++
				report.WaitedList = appendListed(report.WaitedList, trial)
			}
		}
		if err := client.FTDropIndexWithArgs(ctx, table, &redis.FTDropIndexOptions{DeleteDocs: true}).Err(); err != nil {
			log.Fatal(err)
		}
		if err := db.Close(); err != nil {
			log.Fatal(err)
		}
	}
	return report
}

// writeReport writes a report as JSON to the file, or to stdout when it has no name.
func writeReport(out string, report any) {
	encoded, err := json.MarshalIndent(report, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if out == "" {
		fmt.Println(string(encoded))
		return
	}
	if err := os.WriteFile(out, append(encoded, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}

// appendListed keeps the first trial numbers of a count.
func appendListed(list []int, trial int) []int {
	if len(list) < listed {
		list = append(list, trial)
	}
	return list
}

// waitIndexed waits until RediSearch has indexed every document: FT.INFO
// reports percent_indexed 1 and indexing 0. It reports false when the index is
// still indexing after ten seconds, which a backlog on the server causes; the
// probes count that as a wait timeout, not as a result.
func waitIndexed(ctx context.Context, client *redis.Client, index string) bool {
	deadline := time.Now().Add(10 * time.Second)
	for {
		info, err := client.FTInfo(ctx, index).Result()
		if err != nil {
			log.Fatal(err)
		}
		if info.PercentIndexed == 1 && info.Indexing == 0 {
			return true
		}
		if time.Now().After(deadline) {
			return false
		}
		time.Sleep(5 * time.Millisecond)
	}
}

// encodeCategory is Go's encodeCategory in storage/cache/redis.go.
func encodeCategory(category string) string {
	return base64.RawStdEncoding.EncodeToString([]byte("_" + category))
}

// escape is Go's escape in storage/cache/redis.go.
func escape(s string) string {
	r := strings.NewReplacer(
		"-", "\\-",
		":", "\\:",
		".", "\\.",
		"/", "\\/",
		"+", "\\+",
	)
	return r.Replace(s)
}
