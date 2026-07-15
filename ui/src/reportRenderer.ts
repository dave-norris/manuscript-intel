// reportRenderer.ts — Renders structured JSON report data to HTML.
// Each schema maps to a template function. Legacy markdown gets a fallback.

interface ReportEnvelope {
  doc_type: string;
  label: string;
  format: string;   // "json" | "markdown"
  content: string;
  generated_at: string;
}

// ── Public API ────────────────────────────────────────────────────────────────

export function renderReport(envelope: ReportEnvelope, storyName: string): string {
  const timestamp = new Date(envelope.generated_at).toLocaleString();
  const header = `
    <div class="report-header">
      <h1>${esc(storyName)}</h1>
      <h2>${esc(envelope.label)}</h2>
      <div class="report-meta">Generated: ${esc(timestamp)}</div>
    </div>
  `;

  let body: string;
  if (envelope.format === 'json') {
    const data = JSON.parse(envelope.content);
    body = renderBySchema(data, envelope.doc_type);
  } else {
    // Legacy markdown fallback — wrap in a simple pre/formatted block
    body = `<div class="report-markdown">${markdownToHtml(envelope.content)}</div>`;
  }

  return header + body;
}

// ── Schema dispatcher ─────────────────────────────────────────────────────────

function renderBySchema(data: any, docType: string): string {
  const schema = data.schema || '';

  if (schema === 'kdp_keywords_v1') return renderKdpKeywords(data);
  if (schema === 'mi_search_terms_v1') return renderSearchTerms(data);
  if (schema === 'genre_analysis_v1') return renderGenreAnalysis(data);
  if (schema === 'full_report_v1') return renderFullReport(data);
  if (schema === 'category_finder_v1') return renderCategoryFinder(data);
  if (schema === 'category_finder_standalone_v1') return renderCategoryFinderStandalone(data);
  if (schema === 'competition_report_v1') return renderCompetitionReport(data);
  if (schema === 'review_mining_v1') return renderReviewMining(data);
  if (schema === 'author_analysis_v1') return renderAuthorAnalysis(data);
  if (schema === 'analysis_v1') return renderCombinedAnalysis(data);
  if (schema === 'kdp_paste_v1') return renderKdpPaste(data);
  if (schema === 'chapter_summaries_v1') return renderChapterSummaries(data);
  if (schema === 'keyword_search_v1') return renderKeywordSearch(data);
  if (schema === 'genre_ranking_v1') return renderGenreRanking(data);
  if (schema === 'discovery_keywords_v1') return renderDiscoveryKeywords(data);
  if (schema === 'activity_log_v1') return renderActivityLog(data);
  if (schema === 'zeigarnik_v1') return renderZeigarnik(data);

  // Detect BISAC classification by structure or doc_type
  if (docType === 'bisac_classification' || (data.ebook && Array.isArray(data.ebook))) {
    return renderBisacSection(data);
  }

  // Unknown schema — dump as formatted JSON
  return `<pre class="report-json">${esc(JSON.stringify(data, null, 2))}</pre>`;
}

// ── KDP Keywords ──────────────────────────────────────────────────────────────

function renderKdpKeywords(data: any): string {
  const entries = data.entries || [];
  const strategy = data.strategy || '';
  const sourceNote = data.source_note || '';

  let html = '';
  if (sourceNote) html += `<p class="report-note">${esc(sourceNote)}</p>`;

  html += `
    <section class="report-section">
      <h3>Your 7 KDP Keyword Strings</h3>
      <p class="report-hint">Ready to paste directly into KDP's keyword fields. Each must be 50 characters or fewer.</p>
      <table class="report-table">
        <thead><tr><th>#</th><th>Keyword String</th><th>Chars</th><th>Rationale</th></tr></thead>
        <tbody>
  `;
  for (const e of entries) {
    const overClass = e.over_limit ? ' class="over-limit"' : '';
    html += `<tr${overClass}>
      <td>${e.field}</td>
      <td class="keyword-cell">${esc(e.string)}</td>
      <td>${e.chars}</td>
      <td>${esc(e.rationale)}</td>
    </tr>`;
  }
  html += `</tbody></table></section>`;

  if (strategy) {
    html += `
      <section class="report-section">
        <h3>Strategy</h3>
        <p>${esc(strategy)}</p>
      </section>
    `;
  }

  html += `
    <section class="report-section">
      <h3>How to Use</h3>
      <ol>
        <li>Go to KDP &rarr; Your Books &rarr; Edit eBook Details</li>
        <li>Scroll to <strong>Keywords</strong> (7 fields)</li>
        <li>Paste one string per field</li>
        <li>Do NOT use commas inside a field</li>
        <li>Do NOT repeat words already in your title, subtitle, or categories</li>
      </ol>
    </section>
  `;

  return html;
}

// ── Search Terms ───────────────────────────────────────────────────────────────

function renderSearchTerms(data: any): string {
  const keywords: string[] = data.keywords || [];

  let html = `
    <section class="report-section">
      <h3>Competition Search Terms</h3>
      <p class="report-hint">These short phrases are used for competition analysis — finding competing books in the same niche on Amazon. They are NOT the same as your KDP keyword strings.</p>
      <table class="report-table">
        <thead><tr><th>#</th><th>Search Phrase</th></tr></thead>
        <tbody>
  `;
  keywords.forEach((kw, i) => {
    html += `<tr><td>${i + 1}</td><td class="keyword-cell">${esc(kw)}</td></tr>`;
  });
  html += `</tbody></table></section>`;

  return html;
}

// ── Genre Analysis ────────────────────────────────────────────────────────────

function renderGenreAnalysis(data: any): string {
  let html = `<p class="report-note">Classifications are based on AI training data. Verify KDP paths via Canopy or Amazon.</p>`;

  // Industry classification
  html += `<section class="report-section"><h3>Industry Genre Classification</h3>`;
  html += `<div class="two-col">`;
  html += `<div><h4>Ebook</h4><p class="genre-primary">${esc(data.industry_ebook)}</p>`;
  if (data.comps_ebook?.length) {
    html += `<p><strong>Comparable titles:</strong></p><ul>`;
    for (const c of data.comps_ebook) html += `<li>${esc(c)}</li>`;
    html += `</ul>`;
  }
  html += `<p><strong>Reader demographic:</strong> ${esc(data.reader_demographic)}</p></div>`;
  html += `<div><h4>Print</h4><p class="genre-primary">${esc(data.industry_print)}</p>`;
  html += `<p><strong>Bookstore shelving:</strong> ${esc(data.bookstore_shelving)}</p>`;
  if (data.comps_print?.length) {
    html += `<p><strong>Comparable titles:</strong></p><ul>`;
    for (const c of data.comps_print) html += `<li>${esc(c)}</li>`;
    html += `</ul>`;
  }
  html += `</div></div></section>`;

  // KDP Categories
  html += `<section class="report-section"><h3>KDP Category Recommendations</h3>`;
  html += `<div class="two-col">`;
  html += `<div><h4>Kindle Ebook</h4><ul class="category-list">`;
  for (const p of data.kdp_ebook || []) html += `<li><code>${esc(p)}</code></li>`;
  html += `</ul></div>`;
  html += `<div><h4>KDP Print</h4><ul class="category-list">`;
  for (const p of data.kdp_print || []) html += `<li><code>${esc(p)}</code></li>`;
  html += `</ul></div></div></section>`;

  // Genre Signals
  if (data.genre_signals) {
    html += `<section class="report-section"><h3>Genre Signals Summary</h3><p>${esc(data.genre_signals)}</p></section>`;
  }

  // Marketing Notes
  if (data.marketing_notes?.length) {
    html += `<section class="report-section"><h3>Marketing Notes</h3><ul>`;
    for (const n of data.marketing_notes) html += `<li>${esc(n)}</li>`;
    html += `</ul></section>`;
  }

  return html;
}

// ── Full Report ───────────────────────────────────────────────────────────────

function renderFullReport(data: any): string {
  let html = '';
  if (data.genre_analysis) {
    html += renderGenreAnalysis(data.genre_analysis);
  }
  if (!data.competition_done) {
    html += `<p class="report-note">Run <strong>Analyze Competition</strong> to add market data.</p>`;
  }
  return html;
}

// ── Category Finder (story-bound) ─────────────────────────────────────────────

function renderCategoryFinder(data: any): string {
  let html = '';
  if (data.method) {
    html += `<p class="report-hint">${esc(data.method)}</p>`;
  }

  const stores: any[] = data.stores || [];
  for (const store of stores) {
    html += renderStoreSection(store);
  }
  return html;
}

function renderStoreSection(store: any): string {
  let html = `<section class="report-section"><h3>${esc(store.store)}</h3>`;

  // Per-genre results
  const perGenre: any[] = store.per_genre || [];
  if (perGenre.length) {
    html += `<h4>Per-Genre Results</h4>`;
    for (const g of perGenre) {
      html += `<div class="genre-block"><strong>${esc(g.genre)} (${g.confidence}%)</strong>`;
      if (!g.picks?.length) {
        html += `<p class="muted">No confident catalog match.</p>`;
      } else {
        html += `<ul>`;
        for (const p of g.picks) {
          html += `<li><code>${esc(p.path)}</code> (${p.confidence}% match) — ${esc(p.reason)}</li>`;
        }
        html += `</ul>`;
      }
      html += `</div>`;
    }
  }

  // Final categories
  const finals: any[] = store.final_categories || [];
  if (finals.length) {
    html += `<h4>Best for Discoverability</h4>`;
    html += `<table class="report-table"><thead><tr>
      <th>#</th><th>Category Path</th><th>Fit</th><th>Sales to #10</th><th>Publisher</th><th>KU</th><th>Matched By</th>
    </tr></thead><tbody>`;
    for (const q of finals) {
      const bonus = q.is_bonus ? ' <span class="badge">bonus</span>' : '';
      const verified = q.verified
        ? `${esc(q.sales_to_ten)}`
        : '<span class="muted">unverified</span>';
      html += `<tr>
        <td>${q.rank}${bonus}</td>
        <td><code>${esc(q.path)}</code></td>
        <td>${q.fit_confidence}%</td>
        <td>${verified}</td>
        <td>${q.verified ? esc(q.publisher_pct) : '—'}</td>
        <td>${q.verified ? esc(q.ku_pct) : '—'}</td>
        <td>${esc((q.agreeing_genres || []).join(', '))}</td>
      </tr>`;
    }
    html += `</tbody></table>`;
  } else {
    html += `<p class="muted">No candidates cleared the fit bar for this store.</p>`;
  }

  html += `</section>`;
  return html;
}

// ── Category Finder Standalone ────────────────────────────────────────────────

function renderCategoryFinderStandalone(data: any): string {
  let html = `<p class="report-hint">Genre: ${esc(data.genre)} | Store: ${esc(data.store)}</p>`;

  const matched: any[] = data.matched || [];
  if (matched.length) {
    html += `<section class="report-section"><h3>Matched Categories</h3>`;
    html += `<table class="report-table"><thead><tr>
      <th>Category Path</th><th>Match</th><th>Sales to #1</th><th>Sales to #10</th><th>Publisher</th><th>KU</th>
    </tr></thead><tbody>`;
    for (const r of matched) {
      html += `<tr>
        <td><code>${esc(r.path)}</code></td><td>${r.confidence}%</td>
        <td>${esc(r.sales_to_one)}</td><td>${esc(r.sales_to_ten)}</td>
        <td>${esc(r.publisher_pct)}</td><td>${esc(r.ku_pct)}</td>
      </tr>`;
    }
    html += `</tbody></table>`;
    // Keywords per matched category
    for (const r of matched) {
      if (r.keywords) {
        html += `<div class="keywords-block"><h4>${esc(r.path)}</h4><pre>${esc(r.keywords)}</pre></div>`;
      }
    }
    html += `</section>`;
  }

  const considered: any[] = data.considered || [];
  if (considered.length) {
    html += `<section class="report-section"><h3>Also Considered (below 80%)</h3><ol>`;
    for (const r of considered) {
      html += `<li>${esc(r.path)} — <strong>${r.confidence}%</strong></li>`;
    }
    html += `</ol></section>`;
  }

  const failed: any[] = data.failed || [];
  if (failed.length) {
    html += `<section class="report-section"><h3>Search Failures</h3><ul>`;
    for (const r of failed) {
      html += `<li>${esc(r.path)}: ${esc(r.note || 'unknown error')}</li>`;
    }
    html += `</ul></section>`;
  }

  return html;
}

// ── Competition Report ────────────────────────────────────────────────────────

function renderCompetitionReport(data: any): string {
  if (data.content_format === 'markdown') {
    return `<div class="report-markdown">${markdownToHtml(data.content)}</div>`;
  }
  return `<pre>${esc(JSON.stringify(data, null, 2))}</pre>`;
}

// ── Review Mining Report ──────────────────────────────────────────────────────

function renderReviewMining(data: any): string {
  let html = '';
  const books: any[] = data.books_analyzed || [];
  if (books.length) {
    html += `<p class="report-hint">Based on ${data.total_reviews || '?'} reviews from ${books.length} competitor book${books.length > 1 ? 's' : ''}:</p>`;
    html += `<ul class="category-list">`;
    for (const b of books) html += `<li>${esc(b.title)}</li>`;
    html += `</ul>`;
  }
  if (data.content_format === 'markdown' && data.content) {
    html += `<div class="report-markdown">${markdownToHtml(data.content)}</div>`;
  }
  return html;
}

// ── Author Catalog Analysis Report ────────────────────────────────────────────

function renderAuthorAnalysis(data: any): string {
  let html = '';
  const authors: any[] = data.authors_analyzed || [];
  if (authors.length) {
    html += `<section class="report-section"><h3>Authors Analyzed</h3>`;
    html += `<table class="report-table"><thead><tr><th>Author</th><th>Books</th></tr></thead><tbody>`;
    for (const a of authors) {
      html += `<tr><td><strong>${esc(a.name)}</strong></td><td>${a.book_count}</td></tr>`;
    }
    html += `</tbody></table></section>`;
  }
  if (data.content_format === 'markdown' && data.content) {
    html += `<div class="report-markdown">${markdownToHtml(data.content)}</div>`;
  }
  return html;
}

// ── Combined Analysis Report ──────────────────────────────────────────────────

function renderCombinedAnalysis(data: any): string {
  const sections = data.sections || {};
  let html = '';

  // KDP Paste section
  if (sections.kdp_paste) {
    try {
      const paste = JSON.parse(sections.kdp_paste);
      html += renderKdpPaste(paste);
    } catch { html += renderRawSection('KDP Paste', sections.kdp_paste); }
  }

  // Genre ranking
  if (sections.genre_ranking) {
    try {
      const ranking = JSON.parse(sections.genre_ranking);
      html += renderGenreRanking(ranking);
    } catch { html += renderRawSection('Genre Ranking', sections.genre_ranking); }
  }

  // KDP Categories
  if (sections.kdp_categories) {
    try {
      const cats = JSON.parse(sections.kdp_categories);
      html += renderKdpCategoriesSection(cats);
    } catch { html += renderRawSection('KDP Categories', sections.kdp_categories); }
  }

  // BISAC
  if (sections.bisac) {
    try {
      const bisac = JSON.parse(sections.bisac);
      html += renderBisacSection(bisac);
    } catch { html += renderRawSection('BISAC Classification', sections.bisac); }
  }

  // KDP Keywords
  if (sections.kdp_keywords) {
    try {
      const kw = JSON.parse(sections.kdp_keywords);
      html += renderKdpKeywords(kw);
    } catch { html += renderRawSection('KDP Keywords', sections.kdp_keywords); }
  }

  // Discovery keywords
  if (sections.discovery_keywords) {
    try {
      const dk = JSON.parse(sections.discovery_keywords);
      html += renderDiscoveryKeywords(dk);
    } catch { html += renderRawSection('Discovery Keywords', sections.discovery_keywords); }
  }

  // Positioning
  if (sections.positioning) {
    try {
      const pos = JSON.parse(sections.positioning);
      html += renderPositioning(pos);
    } catch { html += renderRawSection('Positioning Context', sections.positioning); }
  }

  return html;
}

// ── Genre Ranking Section ─────────────────────────────────────────────────────

function renderGenreRanking(data: any): string {
  const genres: any[] = data.genres || [];
  if (!genres.length) return '';

  let html = `<section class="report-section"><h3>Genre Ranking</h3>`;
  html += `<p class="report-hint">Scored independently — percentages do not sum to 100.</p>`;
  html += `<table class="report-table"><thead><tr><th>Genre</th><th>Confidence</th><th>Reasoning</th></tr></thead><tbody>`;
  for (const g of genres) {
    html += `<tr>
      <td><strong>${esc(g.genre)}</strong></td>
      <td>${g.confidence}%</td>
      <td>${esc(g.reason)}</td>
    </tr>`;
  }
  html += `</tbody></table></section>`;
  return html;
}

// ── KDP Categories Section (combined report) ──────────────────────────────────

function renderKdpCategoriesSection(data: any): string {
  const stores: any[] = data.stores || [];
  if (!stores.length) return '';

  let html = `<section class="report-section"><h3>KDP Categories</h3>`;
  for (const store of stores) {
    html += `<h4>${esc(store.store)}</h4>`;
    if (store.error) {
      html += `<p class="muted">${esc(store.error)}</p>`;
      continue;
    }
    const cats: any[] = store.categories || [];
    if (!cats.length) {
      html += `<p class="muted">No candidates cleared the fit bar for this store.</p>`;
      continue;
    }
    html += `<table class="report-table"><thead><tr>
      <th>#</th><th>Category Path</th><th>Fit</th><th>Sales to #10</th><th>Matched By</th>
    </tr></thead><tbody>`;
    for (const c of cats) {
      const bonus = c.is_bonus ? ' <span class="badge">bonus</span>' : '';
      const sales = c.verified ? esc(c.sales_to_ten) : '<span class="muted">unverified</span>';
      html += `<tr>
        <td>${c.rank}${bonus}</td>
        <td><code>${esc(c.path)}</code></td>
        <td>${c.fit_confidence}%</td>
        <td>${sales}</td>
        <td>${esc((c.agreeing_genres || []).join(', '))}</td>
      </tr>`;
      // Top bestsellers for this category
      const books: any[] = c.top_books || [];
      if (books.length) {
        html += `<tr><td colspan="5" class="top-books-cell">`;
        for (const b of books) {
          const img = b.image_url ? `<img src="${esc(b.image_url)}" height="50" /> ` : '';
          html += `<a href="https://www.amazon.com/dp/${esc(b.asin)}" class="top-book-link">${img}${esc(b.title)}</a>`;
        }
        html += `</td></tr>`;
      }
    }
    html += `</tbody></table>`;
  }
  html += `</section>`;
  return html;
}

// ── BISAC Section ─────────────────────────────────────────────────────────────

function renderBisacSection(data: any): string {
  let html = `<section class="report-section"><h3>BISAC Classification</h3>`;
  html += `<p class="report-hint">Verify against BISG's free lookup (bisg.org/complete-bisac-subject-headings-list) before submitting. Kindle eBook no longer takes BISAC directly on KDP — this matters for KDP Print and wide/Ingram distribution.</p>`;

  const ebook: any[] = data.ebook || [];
  html += `<h4>Ebook</h4>`;
  if (!ebook.length) {
    html += `<p class="muted">No confident BISAC match.</p>`;
  } else {
    html += `<table class="report-table"><thead><tr><th>Code</th><th>Heading</th><th>Confidence</th><th>Reasoning</th></tr></thead><tbody>`;
    for (const b of ebook) {
      html += `<tr><td><code>${esc(b.code)}</code></td><td>${esc(b.heading)}</td><td>${b.confidence}%</td><td>${esc(b.reason || '')}</td></tr>`;
    }
    html += `</tbody></table>`;
  }

  html += `<h4>Print</h4>`;
  if (data.print === 'same_as_ebook') {
    html += `<p class="muted">Same as ebook.</p>`;
  } else {
    const print: any[] = data.print || [];
    if (!print.length) {
      html += `<p class="muted">No confident BISAC match.</p>`;
    } else {
      html += `<table class="report-table"><thead><tr><th>Code</th><th>Heading</th><th>Confidence</th><th>Reasoning</th></tr></thead><tbody>`;
      for (const b of print) {
        html += `<tr><td><code>${esc(b.code)}</code></td><td>${esc(b.heading)}</td><td>${b.confidence}%</td><td>${esc(b.reason || '')}</td></tr>`;
      }
      html += `</tbody></table>`;
    }
  }

  html += `</section>`;
  return html;
}

// ── Discovery Keywords Section ────────────────────────────────────────────────

function renderDiscoveryKeywords(data: any): string {
  const keywords: any[] = data.keywords || [];
  if (!keywords.length) return '';

  let html = `<section class="report-section"><h3>Discovery Keywords</h3>`;
  html += `<p class="report-hint">Optimized for Apple Books, Kobo, Google Play, B&N, BookBub, Goodreads, and SEO.</p>`;
  html += `<table class="report-table"><thead><tr><th>Phrase</th><th>Rationale</th></tr></thead><tbody>`;
  for (const k of keywords) {
    html += `<tr><td class="keyword-cell">${esc(k.phrase)}</td><td>${esc(k.rationale)}</td></tr>`;
  }
  html += `</tbody></table></section>`;
  return html;
}

// ── Positioning Section ───────────────────────────────────────────────────────

function renderPositioning(data: any): string {
  let html = `<section class="report-section"><h3>Positioning Context</h3>`;
  html += `<table class="report-table"><tbody>`;
  html += `<tr><td><strong>Reader demographic</strong></td><td>${esc(data.reader_demographic || '')}</td></tr>`;
  html += `<tr><td><strong>Bookstore shelving</strong></td><td>${esc(data.bookstore_shelving || '')}</td></tr>`;
  html += `</tbody></table>`;

  const compsEbook: string[] = data.comps_ebook || [];
  const compsPrint: string[] = data.comps_print || [];
  if (compsEbook.length || compsPrint.length) {
    html += `<div class="two-col">`;
    if (compsEbook.length) {
      html += `<div><h4>Ebook Comps</h4><ul>`;
      for (const c of compsEbook) html += `<li>${esc(c)}</li>`;
      html += `</ul></div>`;
    }
    if (compsPrint.length) {
      html += `<div><h4>Print Comps</h4><ul>`;
      for (const c of compsPrint) html += `<li>${esc(c)}</li>`;
      html += `</ul></div>`;
    }
    html += `</div>`;
  }

  html += `</section>`;
  return html;
}

// ── KDP Paste Section ─────────────────────────────────────────────────────────

function renderKdpPaste(data: any): string {
  let html = `<section class="report-section"><h3>KDP Metadata &mdash; Ready to Paste</h3>`;

  html += `<div class="two-col">`;
  html += `<div><h4>Categories (Kindle eBook)</h4><ol class="category-list">`;
  for (const c of data.kindle_categories || []) html += `<li><code>${esc(c)}</code></li>`;
  html += `</ol></div>`;
  html += `<div><h4>Categories (Paperback)</h4><ol class="category-list">`;
  for (const c of data.print_categories || []) html += `<li><code>${esc(c)}</code></li>`;
  html += `</ol></div></div>`;

  const keywords: string[] = data.keywords || [];
  if (keywords.length) {
    html += `<h4>Keywords</h4>`;
    html += `<table class="report-table keywords-paste-table"><thead><tr><th>Field 1–4</th><th>Field 5–7</th></tr></thead><tbody>`;
    for (let i = 0; i < 4; i++) {
      const left = keywords[i] || '';
      const right = keywords[i + 4] || '';
      html += `<tr><td>${esc(left)}</td><td>${esc(right)}</td></tr>`;
    }
    html += `</tbody></table>`;
  }

  html += `</section>`;
  return html;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// ── Chapter Summaries Report ──────────────────────────────────────────────────

function renderChapterSummaries(data: any): string {
  const chapters: any[] = data.chapters || [];
  if (!chapters.length) return '<p class="muted">No chapter summaries available.</p>';

  const totalWords = data.total_words || chapters.reduce((sum: number, c: any) => sum + (c.word_count || 0), 0);
  let html = `<p class="report-hint">${chapters.length} chapters, ${totalWords.toLocaleString()} total words</p>`;
  html += `<table class="report-table"><thead><tr><th>#</th><th>Chapter</th><th>Words</th><th>Genre Signals</th></tr></thead><tbody>`;
  chapters.forEach((ch, i) => {
    html += `<tr>
      <td>${i + 1}</td>
      <td><strong>${esc(ch.title || ch.file)}</strong></td>
      <td>${(ch.word_count || 0).toLocaleString()}</td>
      <td>${esc(ch.signals || '').substring(0, 200)}${(ch.signals || '').length > 200 ? '...' : ''}</td>
    </tr>`;
  });
  html += `</tbody></table>`;
  return html;
}

// ── Keyword Search Results Report ─────────────────────────────────────────────

function renderKeywordSearch(data: any): string {
  const keywords: any[] = data.keywords || [];
  if (!keywords.length) return '<p class="muted">No keyword search results available.</p>';

  let html = `<p class="report-hint">${keywords.length} keywords analyzed</p>`;
  html += `<table class="report-table"><thead><tr><th>Keyword</th><th>Est. Monthly Searches</th><th>Competition</th><th>Est. Earnings</th></tr></thead><tbody>`;
  for (const k of keywords) {
    const compClass = k.competition === 'Low' ? 'style="color:#27ae60"' :
                      k.competition === 'High' ? 'style="color:#e74c3c"' : '';
    html += `<tr>
      <td class="keyword-cell">${esc(k.keyword)}</td>
      <td>${esc(k.searches)}</td>
      <td ${compClass}><strong>${esc(k.competition)}</strong></td>
      <td>${esc(k.earnings)}</td>
    </tr>`;
  }
  html += `</tbody></table>`;
  return html;
}

function renderRawSection(title: string, content: string): string {
  // For sections that are still plain text (not yet converted to JSON)
  return `<section class="report-section"><h3>${esc(title)}</h3><pre class="report-raw">${esc(content)}</pre></section>`;
}

/** Minimal markdown to HTML — handles headers, bold, italic, code, lists, hr */
function markdownToHtml(md: string): string {
  return md
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/^### (.+)$/gm, '<h4>$1</h4>')
    .replace(/^## (.+)$/gm, '<h3>$1</h3>')
    .replace(/^# (.+)$/gm, '<h2>$1</h2>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    .replace(/`(.+?)`/g, '<code>$1</code>')
    .replace(/^- (.+)$/gm, '<li>$1</li>')
    .replace(/(<li>.*<\/li>\n?)+/g, (m) => `<ul>${m}</ul>`)
    .replace(/^---$/gm, '<hr>')
    .replace(/^> (.+)$/gm, '<blockquote>$1</blockquote>')
    .replace(/\n{2,}/g, '</p><p>')
    .replace(/^/, '<p>').replace(/$/, '</p>')
    .replace(/<p><\/p>/g, '')
    .replace(/<p>(<h[2-4]>)/g, '$1')
    .replace(/(<\/h[2-4]>)<\/p>/g, '$1')
    .replace(/<p>(<ul>)/g, '$1')
    .replace(/(<\/ul>)<\/p>/g, '$1')
    .replace(/<p>(<hr>)<\/p>/g, '$1')
    .replace(/<p>(<blockquote>)/g, '$1')
    .replace(/(<\/blockquote>)<\/p>/g, '$1');
}

// ── Zeigarnik Effect Report ───────────────────────────────────────────────────

function renderZeigarnik(data: any): string {
  const summary = data.summary || {};
  const chapters: any[] = data.chapters || [];
  const threads: any[] = data.threads || [];

  let html = '';
  if (data.note) {
    html += `<p class="report-note">${esc(data.note)}</p>`;
  }

  html += `<section class="report-section"><h3>Summary</h3>`;
  html += `<table class="report-table"><tbody>`;
  html += `<tr><td><strong>Chapters analyzed</strong></td><td>${summary.total_chapters ?? '—'}</td></tr>`;
  html += `<tr><td><strong>Total words</strong></td><td>${(summary.total_words ?? 0).toLocaleString()}</td></tr>`;
  html += `<tr><td><strong>Cliffhanger endings</strong></td><td>${summary.cliffhanger_endings ?? 0} of ${summary.total_chapters ?? 0} (${summary.cliffhanger_pct ?? 0}%)</td></tr>`;
  html += `<tr><td><strong>Resolved endings</strong></td><td>${summary.resolved_endings ?? 0}</td></tr>`;
  html += `<tr><td><strong>Open narrative questions</strong></td><td>${summary.total_open_questions ?? 0}</td></tr>`;
  html += `<tr><td><strong>Average tension score</strong></td><td>${summary.avg_tension_score ?? 0} / 100</td></tr>`;
  html += `<tr><td><strong>Candidate open threads</strong></td><td>${summary.open_thread_count ?? 0}</td></tr>`;
  html += `<tr><td><strong>Longest thread gap</strong></td><td>${summary.longest_gap_chapters ?? 0} chapters</td></tr>`;
  html += `</tbody></table></section>`;

  if (threads.length) {
    html += `<section class="report-section"><h3>Open Threads</h3>`;
    html += `<p class="report-hint">A capitalized name, place, or object that appears, then goes quiet for several chapters, then resurfaces — the textual shape of an unresolved thread. Sorted by the longest gap.</p>`;
    html += `<table class="report-table"><thead><tr><th>Term</th><th>First Seen</th><th>Gap</th><th>Resurfaces</th><th>Mentions</th><th>Context</th></tr></thead><tbody>`;
    for (const t of threads) {
      html += `<tr>
        <td><strong>${esc(t.term)}</strong></td>
        <td>Ch. ${(t.first_chapter_index ?? 0) + 1}</td>
        <td>${t.max_gap_chapters} chapters / ${(t.max_gap_words ?? 0).toLocaleString()} words</td>
        <td>Ch. ${(t.gap_end_index ?? 0) + 1}</td>
        <td>${t.mention_count}</td>
        <td class="muted">${esc(t.first_snippet || '')}</td>
      </tr>`;
    }
    html += `</tbody></table></section>`;
  } else {
    html += `<section class="report-section"><p class="muted">No long-gap threads found above the configured threshold.</p></section>`;
  }

  if (chapters.length) {
    html += `<section class="report-section"><h3>Chapter Endings</h3>`;
    html += `<table class="report-table"><thead><tr><th>#</th><th>Chapter</th><th>Words</th><th>Questions</th><th>Ending</th><th>Tension</th></tr></thead><tbody>`;
    chapters.forEach((c: any, i: number) => {
      const endingClass = c.ending_type === 'cliffhanger' ? 'style="color:#e74c3c"' :
                          c.ending_type === 'resolved' ? 'style="color:#27ae60"' : '';
      html += `<tr>
        <td>${i + 1}</td>
        <td><strong>${esc(c.title || c.file)}</strong></td>
        <td>${(c.word_count ?? 0).toLocaleString()}</td>
        <td>${c.question_count ?? 0}</td>
        <td ${endingClass}><strong>${esc(c.ending_type)}</strong></td>
        <td>${c.tension_score ?? 0}</td>
      </tr>`;
      if (c.ending_snippet) {
        html += `<tr><td colspan="6" class="top-books-cell muted">"${esc(c.ending_snippet)}"</td></tr>`;
      }
    });
    html += `</tbody></table></section>`;
  }

  return html;
}

function renderActivityLog(data: any): string {
  const lines: any[] = data.lines || [];
  if (!lines.length) return '<p class="muted">No log entries.</p>';

  const timestamp = data.timestamp ? new Date(data.timestamp).toLocaleString() : '';
  let html = '';
  if (timestamp) {
    html += `<p class="report-hint">Recorded: ${esc(timestamp)}</p>`;
  }

  html += `<div class="log-stream">`;
  for (const line of lines) {
    const typeClass = esc(line.type || 'log-info');
    const icon = line.icon || '';
    const text = esc(line.text || '').replace(/`([^`]+)`/g, '<code>$1</code>');
    html += `<div class="log-line ${typeClass}">`;
    if (icon) html += `<span class="log-icon">${esc(icon)}</span>`;
    html += `<span class="log-text">${text}</span></div>`;
  }
  html += `</div>`;
  return html;
}

function esc(s: string): string {
  if (!s) return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
