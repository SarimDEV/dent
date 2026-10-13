# Moving invoices to the new ledger

Invoices are written to two tables today, and they drift whenever a retry lands between the writes. This plan moves every invoice to the `ledger_entries` table in four steps, each one safe to roll back.

## What changes

- New invoices write only to `ledger_entries`, inside one transaction.
- A backfill copies the 2.1 million old invoices in batches of 5,000, oldest first.
- Reports read from a view that unions both tables until the backfill finishes.
- The old `invoices` table becomes read-only, then is dropped after 30 days.

## Steps

1. **Dual read.** Ship the union view behind `ledger_read`. No writes change.
2. **Single write.** Flip `ledger_write` for 5% of accounts, then 50%, then everyone. Each step waits a full billing day.
3. **Backfill.** Run `billing-backfill --batch 5000 --sleep 200ms` overnight. It is idempotent and resumes from the last id.
4. **Retire.** Revoke writes on `invoices`, keep it 30 days, then drop it.

## Risks

> A retry that lands during step 2 can still write to the old table. The write path checks the flag inside the transaction, so a retry sees the same answer as the first attempt.

| Risk | Likelihood | What we do |
| --- | --- | --- |
| Backfill slows the primary | Medium | Batches sleep 200 ms; pause with one flag |
| Totals differ after the move | Low | Nightly check compares sums per account |
| A report reads the wrong table | Low | Every report goes through the view |

## Checklist

- [x] Union view written and tested
- [x] Flags created, both off
- [ ] Backfill dry run on staging
- [ ] Sign-off from finance

```sql
CREATE VIEW invoices_all AS
  SELECT id, account_id, total_cents, issued_at FROM ledger_entries
  UNION ALL
  SELECT id, account_id, total_cents, issued_at FROM invoices
  WHERE id NOT IN (SELECT id FROM ledger_entries);
```
