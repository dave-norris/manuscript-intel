// Package rocket provides the high-level Publisher Rocket automation service.
// It wraps the cdp package and exposes methods that map to the VS Code
// extension's rocket-tools.ts commands.
package rocket

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"pub-rocket-reader/internal/cdp"

	"github.com/wailsapp/wails/v2/pkg/runtime"
)

// AnalyzerResult is returned by all analyzer methods and surfaced to the frontend.
type AnalyzerResult struct {
	Success  bool   `json:"success"`
	Markdown string `json:"markdown,omitempty"`
	Error    string `json:"error,omitempty"`
}

// Service manages a single CDP session lifecycle for the duration the app is open.
type Service struct {
	ctx     context.Context
	session *cdp.Session
}

// NewService creates a new rocket Service.
func NewService(ctx context.Context) *Service {
	return &Service{ctx: ctx}
}

// Cleanup closes the active CDP session if one is open.
func (s *Service) Cleanup() {
	if s.session != nil {
		s.session.Close()
		s.session = nil
	}
}

// log emits a message to the Wails frontend via runtime events so the UI can
// display a live log panel.
func (s *Service) log(msg string) {
	runtime.EventsEmit(s.ctx, "cdp:log", msg)
}

// ── Status / Launch ───────────────────────────────────────────────────────────

// CheckStatus returns the current Publisher Rocket + CDP state.
func (s *Service) CheckStatus() cdp.StatusResult {
	target, err := cdp.GetPageTarget()
	if err != nil {
		portOpen := cdp.IsPortOpen()
		return cdp.StatusResult{
			Running:    portOpen,
			CDPEnabled: false,
			Error:      err.Error(),
		}
	}
	return cdp.StatusResult{
		Running:    true,
		CDPEnabled: true,
		PageID:     target.ID,
	}
}

// EnsureRunning launches Publisher Rocket with CDP if needed and connects a session.
func (s *Service) EnsureRunning() cdp.LaunchResult {
	s.log("Ensuring Publisher Rocket is running with CDP...")

	target, err := cdp.EnsureRocket()
	if err != nil {
		s.log("ERROR: " + err.Error())
		return cdp.LaunchResult{Error: err.Error()}
	}

	s.log(fmt.Sprintf("Found page target: %s (id: %s)", target.Title, target.ID))

	sess, err := cdp.Connect(target)
	if err != nil {
		s.log("ERROR: CDP connect failed: " + err.Error())
		return cdp.LaunchResult{Error: err.Error()}
	}

	s.Cleanup()
	s.session = sess
	s.log("CDP session established.")
	return cdp.LaunchResult{Success: true, PageID: target.ID}
}

// ── Category Analyzer ─────────────────────────────────────────────────────────

// AnalyzeCategories scrapes Publisher Rocket's Category Search UI for each
// category path and returns a markdown report.
//
// categoryPaths is a slice of full path strings, e.g.:
//
//	"Literature & Fiction > Historical Fiction > Ancient World"
func (s *Service) AnalyzeCategories(categoryPaths []string) AnalyzerResult {
	if s.session == nil {
		result := s.EnsureRunning()
		if !result.Success {
			return AnalyzerResult{Error: result.Error}
		}
	}

	lines := []string{"# Category Research", fmt.Sprintf("Generated: %s", time.Now().Format("January 2, 2006 3:04 PM")), ""}

	for i, fullPath := range categoryPaths {
		segments := splitPath(fullPath)
		if len(segments) == 0 {
			continue
		}
		s.log(fmt.Sprintf("[cat %d/%d] %s", i+1, len(categoryPaths), fullPath))

		if err := s.navigateToCategorySearch(); err != nil {
			s.log("WARN: Could not navigate to Category Search: " + err.Error())
		}
		time.Sleep(2 * time.Second)

		if err := s.clickKindleRadio(); err != nil {
			s.log("WARN: Kindle radio click: " + err.Error())
		}
		time.Sleep(800 * time.Millisecond)

		topCat := segments[0]
		if err := s.typeInSearchBox(topCat); err != nil {
			s.log("WARN: Search box: " + err.Error())
		}
		time.Sleep(2 * time.Second)

		if err := s.clickCheckItOut(topCat); err != nil {
			s.log(fmt.Sprintf("SKIP: '%s' not found in results", topCat))
			lines = append(lines, "## "+fullPath, "", "*Not found*", "", "---", "")
			continue
		}
		s.log(fmt.Sprintf("[cat %d] Clicked 'Check it out'", i+1))
		time.Sleep(5 * time.Second)

		target := segments[len(segments)-1]
		row, err := s.scrapeRowData(target)
		if err != nil || row == nil {
			s.log(fmt.Sprintf("SKIP: '%s' not in subcategory table", target))
			lines = append(lines, "## "+fullPath, "", "*Subcategory not found*", "", "---", "")
			s.clickBack()
			time.Sleep(2 * time.Second)
			continue
		}

		lines = append(lines,
			"## "+fullPath, "",
			fmt.Sprintf("- **Sales to #1:** %s", row.SalesToOne),
			fmt.Sprintf("- **Sales to #10:** %s", row.SalesToTen),
			fmt.Sprintf("- **Publisher %%:** %s", row.PublisherPct),
			fmt.Sprintf("- **KU %%:** %s", row.KUPct),
			"",
		)

		if ins := s.scrapeModal(row.InsightsCoords); ins != "" {
			lines = append(lines, "### Insights", "", ins, "")
			s.session.KeyEscape()
		}

		if kw := s.scrapeModal(row.KeywordsCoords); kw != "" {
			lines = append(lines, "### Keywords", "", kw, "")
			s.session.KeyEscape()
		}

		lines = append(lines, "---", "")
		s.clickBack()
		time.Sleep(2 * time.Second)
	}

	return AnalyzerResult{Success: true, Markdown: strings.Join(lines, "\n")}
}

// ── CSV Analyzer ──────────────────────────────────────────────────────────────

// AnalyzeCSV sends CSV content to an AI model and returns a markdown analysis.
// (AI call is a TODO — returns a placeholder until the AI client is wired up.)
func (s *Service) AnalyzeCSV(keyword string, csvContent string) AnalyzerResult {
	// TODO: wire up AI client (Anthropic API or OpenRouter)
	_ = keyword
	_ = csvContent
	return AnalyzerResult{
		Error: "CSV Analyzer: AI client not yet implemented",
	}
}

// ── CDP navigation helpers ────────────────────────────────────────────────────

func (s *Service) navigateToCategorySearch() error {
	js := `
		const el = Array.from(document.querySelectorAll('p,span,div,a'))
			.find(e => e.children.length === 0 && e.textContent.trim() === 'Category Search');
		if (!el) return JSON.stringify(null);
		el.scrollIntoView({block:'center'});
		const r = el.getBoundingClientRect();
		return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
	`
	result, err := s.session.Eval(js, 8*time.Second)
	if err != nil || result == "null" || result == "" {
		return fmt.Errorf("Category Search nav element not found")
	}
	var coords struct{ X, Y float64 }
	if err := json.Unmarshal([]byte(result), &coords); err != nil {
		return err
	}
	return s.session.Click(coords.X, coords.Y)
}

func (s *Service) clickKindleRadio() error {
	js := `
		const el = Array.from(document.querySelectorAll('label,span,p,input'))
			.find(e => e.textContent && e.textContent.trim() === 'Kindle');
		if (!el) return JSON.stringify(null);
		const t = el.closest('label') || el;
		const r = t.getBoundingClientRect();
		return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
	`
	result, err := s.session.Eval(js, 8*time.Second)
	if err != nil || result == "null" || result == "" {
		return fmt.Errorf("Kindle radio not found")
	}
	var coords struct{ X, Y float64 }
	if err := json.Unmarshal([]byte(result), &coords); err != nil {
		return err
	}
	return s.session.Click(coords.X, coords.Y)
}

func (s *Service) typeInSearchBox(value string) error {
	js := fmt.Sprintf(`
		const input = document.querySelector('input[type="text"], input[type="search"], input:not([type="radio"]):not([type="checkbox"])');
		if (input) {
			input.value = '';
			input.dispatchEvent(new Event('input',{bubbles:true}));
			input.value = %s;
			input.dispatchEvent(new Event('input',{bubbles:true}));
			input.dispatchEvent(new Event('change',{bubbles:true}));
		}
		return '';
	`, jsonStr(value))
	_, err := s.session.Eval(js, 8*time.Second)
	return err
}

func (s *Service) clickCheckItOut(topCat string) error {
	js := fmt.Sprintf(`
		const rows = Array.from(document.querySelectorAll('tr'));
		for (const row of rows) {
			const cells = row.querySelectorAll('td');
			if (cells.length > 0 && cells[0].textContent.trim().includes(%s)) {
				const btn = Array.from(row.querySelectorAll('button')).find(b => b.textContent.trim() === 'Check it out');
				if (btn) {
					btn.scrollIntoView({block:'center'});
					const r = btn.getBoundingClientRect();
					return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
				}
			}
		}
		return JSON.stringify(null);
	`, jsonStr(topCat))
	result, err := s.session.Eval(js, 8*time.Second)
	if err != nil || result == "null" || result == "" {
		return fmt.Errorf("Check it out not found for %q", topCat)
	}
	var coords struct{ X, Y float64 }
	if err := json.Unmarshal([]byte(result), &coords); err != nil {
		return err
	}
	return s.session.Click(coords.X, coords.Y)
}

type rowData struct {
	SalesToOne     string
	SalesToTen     string
	PublisherPct   string
	KUPct          string
	InsightsCoords *coords
	KeywordsCoords *coords
}

type coords struct{ X, Y float64 }

func (s *Service) scrapeRowData(target string) (*rowData, error) {
	js := fmt.Sprintf(`
		const target = %s.toLowerCase();
		const rows = Array.from(document.querySelectorAll('tr')).slice(1);
		let bestRow = null, bestScore = 0;
		for (const row of rows) {
			const cells = Array.from(row.querySelectorAll('td'));
			if (cells.length < 2) continue;
			const txt = cells[0] ? cells[0].textContent.replace(/\s+/g,' ').trim() : '';
			const lastSeg = txt.split(/[>\/]/).pop().trim().toLowerCase();
			let score = 0;
			if (lastSeg === target) score = 100;
			else if (lastSeg.startsWith(target) || target.startsWith(lastSeg)) score = 80;
			else if (txt.toLowerCase().includes(target) || target.includes(lastSeg)) score = 60;
			if (score > bestScore) { bestScore = score; bestRow = { row, cells }; }
		}
		if (!bestRow || bestScore === 0) return JSON.stringify(null);
		const { row, cells } = bestRow;
		const btns = Array.from(row.querySelectorAll('button'));
		const iBtn = btns.find(b => b.textContent.trim() === 'Insights');
		const kBtn = btns.find(b => b.textContent.trim() === 'Keywords');
		const ri = iBtn ? iBtn.getBoundingClientRect() : null;
		const rk = kBtn ? kBtn.getBoundingClientRect() : null;
		return JSON.stringify({
			salesToOne:    cells[1] ? cells[1].textContent.trim() : '',
			salesToTen:    cells[2] ? cells[2].textContent.trim() : '',
			publisherPct:  cells[3] ? cells[3].textContent.trim() : '',
			kuPct:         cells[4] ? cells[4].textContent.trim() : '',
			iCoords: ri ? {x:Math.round(ri.x+ri.width/2), y:Math.round(ri.y+ri.height/2)} : null,
			kCoords: rk ? {x:Math.round(rk.x+rk.width/2), y:Math.round(rk.y+rk.height/2)} : null,
		});
	`, jsonStr(target))

	result, err := s.session.Eval(js, 15*time.Second)
	if err != nil || result == "null" || result == "" {
		return nil, fmt.Errorf("row not found")
	}

	var raw struct {
		SalesToOne   string      `json:"salesToOne"`
		SalesToTen   string      `json:"salesToTen"`
		PublisherPct string      `json:"publisherPct"`
		KUPct        string      `json:"kuPct"`
		ICoords      interface{} `json:"iCoords"`
		KCoords      interface{} `json:"kCoords"`
	}
	if err := json.Unmarshal([]byte(result), &raw); err != nil {
		return nil, err
	}

	row := &rowData{
		SalesToOne:   raw.SalesToOne,
		SalesToTen:   raw.SalesToTen,
		PublisherPct: raw.PublisherPct,
		KUPct:        raw.KUPct,
	}

	if raw.ICoords != nil {
		if m, ok := raw.ICoords.(map[string]interface{}); ok {
			row.InsightsCoords = &coords{X: m["x"].(float64), Y: m["y"].(float64)}
		}
	}
	if raw.KCoords != nil {
		if m, ok := raw.KCoords.(map[string]interface{}); ok {
			row.KeywordsCoords = &coords{X: m["x"].(float64), Y: m["y"].(float64)}
		}
	}
	return row, nil
}

func (s *Service) scrapeModal(c *coords) string {
	if c == nil {
		return ""
	}
	if err := s.session.Click(c.X, c.Y); err != nil {
		return ""
	}
	time.Sleep(2 * time.Second)
	js := `
		const o = document.querySelector('[class*="modal"],[class*="overlay"],[class*="popup"]');
		return o ? o.innerText : '';
	`
	result, _ := s.session.Eval(js, 8*time.Second)
	return result
}

func (s *Service) clickBack() {
	js := `
		const el = Array.from(document.querySelectorAll('button,a,span'))
			.find(e => e.textContent.trim() === 'Back' || e.textContent.trim() === '← Back');
		if (!el) return JSON.stringify(null);
		el.scrollIntoView({block:'center'});
		const r = el.getBoundingClientRect();
		return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
	`
	result, err := s.session.Eval(js, 8*time.Second)
	if err != nil || result == "null" || result == "" {
		return
	}
	var c coords
	if err := json.Unmarshal([]byte(result), &c); err == nil {
		s.session.Click(c.X, c.Y)
	}
}

// ── Utilities ─────────────────────────────────────────────────────────────────

// splitPath splits "Literature & Fiction > Historical Fiction > Ancient World"
// into ["Literature & Fiction", "Historical Fiction", "Ancient World"],
// stripping common Kindle Store prefix segments.
func splitPath(path string) []string {
	raw := strings.Split(path, ">")
	var segments []string
	for _, s := range raw {
		s = strings.TrimSpace(s)
		if s == "" {
			continue
		}
		lower := strings.ToLower(s)
		if lower == "books" || lower == "kindle books" || lower == "kindle store" || lower == "kindle" {
			continue
		}
		segments = append(segments, s)
	}
	return segments
}

// jsonStr marshals a Go string to a JSON string literal for embedding in JS.
func jsonStr(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}
