-- Glossary candidates proposed before the summarizer saw the source text often carry the
-- translation as their source (e.g. "Art of Drawing" for "l'Art du Dessin"). Drop every
-- proposed, still-undecided candidate whose source does not occur in the project's source
-- blocks; approved, conflicting, rejected and hand-written terms are left alone.
-- The match ignores ASCII case and typographic apostrophes, like the summarizer's own check.
DELETE FROM glossary_term
WHERE status = 'candidate'
  AND origin = 'proposed'
  AND NOT EXISTS (
    SELECT 1
    FROM block b
    JOIN document d ON d.id = b.document_id
    WHERE d.project_id = glossary_term.project_id
      AND instr(
        lower(replace(b.source_text, '’', '''')),
        lower(replace(glossary_term.source, '’', ''''))
      ) > 0
  );
