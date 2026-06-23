package main

import (
	"context"

	"pub-rocket-reader/internal/cdp"
	"pub-rocket-reader/internal/rocket"
)

// App is the main application struct exposed to the Wails frontend.
// Public methods on this struct become callable from JavaScript via window.go.
type App struct {
	ctx    context.Context
	rocket *rocket.Service
}

// NewApp creates a new App instance.
func NewApp() *App {
	return &App{}
}

// startup is called by Wails when the application starts.
func (a *App) startup(ctx context.Context) {
	a.ctx = ctx
	a.rocket = rocket.NewService(ctx)
}

// shutdown is called by Wails when the application is closing.
func (a *App) shutdown(ctx context.Context) {
	if a.rocket != nil {
		a.rocket.Cleanup()
	}
}

// ── CDP / Rocket status ───────────────────────────────────────────────────────

// CheckRocketStatus returns whether Publisher Rocket is running with CDP enabled.
func (a *App) CheckRocketStatus() cdp.StatusResult {
	return a.rocket.CheckStatus()
}

// LaunchRocket ensures Publisher Rocket is running with CDP on port 9222.
func (a *App) LaunchRocket() cdp.LaunchResult {
	return a.rocket.EnsureRunning()
}

// ── Category Analyzer ────────────────────────────────────────────────────────

// RunCategoryAnalyzer scrapes category data from Publisher Rocket's Category
// Search UI for the given category paths and returns the raw markdown report.
func (a *App) RunCategoryAnalyzer(categoryPaths []string) rocket.AnalyzerResult {
	return a.rocket.AnalyzeCategories(categoryPaths)
}

// ── CSV Analyzer ─────────────────────────────────────────────────────────────

// AnalyzeCSV accepts raw CSV content exported from Publisher Rocket's
// Competition Analyzer and returns an AI-generated markdown analysis.
func (a *App) AnalyzeCSV(keyword string, csvContent string) rocket.AnalyzerResult {
	return a.rocket.AnalyzeCSV(keyword, csvContent)
}
