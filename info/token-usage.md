---
inclusion: manual
---

# Token Usage Estimates

For a 40-chapter manuscript (~4000 words/chapter), running every report once:

## Per-report breakdown

| Report | LLM Calls | Input/call | Output max/call | Total input | Total output |
|--------|-----------|------------|-----------------|-------------|--------------|
| Chapter Summaries | 40 | ~10,700 | 600 | ~428K | ~24K |
| Genre Analysis | 1 | ~2,000 | 1,200 | ~2K | ~1.2K |
| Genre Ranking | 1 | ~3,000 | 1,200 | ~3K | ~1.2K |
| KDP Categories | 2 | ~2,000 | 1,200 | ~4K | ~2.4K |
| BISAC | 1-2 | ~2,000 | 1,200 | ~4K | ~2.4K |
| Search Terms | 1 | ~1,000 | 300 | ~1K | ~0.3K |
| KDP Keywords | 1 | ~3,000 | 1,200 | ~3K | ~1.2K |
| Discovery Keywords | 1 | ~2,000 | 1,200 | ~2K | ~1.2K |
| Continuity Extract | 40 | ~5,500 | 4,000 | ~220K | ~160K |
| Continuity Judge | 1-5 | ~3,000 | 2,500 | ~15K | ~12.5K |
| Show Don't Tell | 40 | ~5,500 | 4,000 | ~220K | ~160K |

## Totals

| Pipeline | Input | Output | Combined |
|----------|-------|--------|----------|
| KDP/Wide | ~447K | ~34K | ~481K tokens |
| Craft: Continuity | ~235K | ~172K | ~407K tokens |
| Craft: Show Don't Tell | ~220K | ~160K | ~380K tokens |
| **ALL REPORTS** | **~902K** | **~366K** | **~1.27M tokens** |

## Cost estimates (40 chapters, all reports)

- Cheap model (Gemini Flash / Qwen): ~$0.10–$0.30
- Mid-tier (Claude Sonnet / GPT-4o): ~$5–$8
- Premium (Claude Opus): ~$25–$40

## Big consumers

- Chapter Summaries: 40 calls x 8000-word input each (largest single report)
- Continuity Extract: 40 calls x 4000-word input
- Show Don't Tell: 40 calls x 4000-word input
- KDP pipeline by itself: only ~481K tokens total (cheap)
