-- `needs_review` now always carries a translation. Chunks flagged before that rule, whose
-- refused answer left them with no text, are what `failed` means today.
UPDATE chunk
SET status = 'failed'
WHERE status = 'needs_review'
  AND (target_md IS NULL OR trim(target_md) = '');
