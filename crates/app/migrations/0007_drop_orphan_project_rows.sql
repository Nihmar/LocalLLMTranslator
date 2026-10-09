-- Rows left behind by projects deleted before `delete_project` removed its dependents:
-- these tables carry `project_id` without a foreign key, and `llm_call` only points at
-- jobs and chunks.
DELETE FROM llm_call
WHERE (job_id IS NOT NULL AND job_id NOT IN (
         SELECT j.id FROM job j WHERE j.project_id IN (SELECT id FROM project)))
   OR (chunk_id IS NOT NULL AND chunk_id NOT IN (SELECT id FROM chunk));
DELETE FROM qa_finding WHERE project_id NOT IN (SELECT id FROM project);
DELETE FROM glossary_term WHERE project_id NOT IN (SELECT id FROM project);
DELETE FROM project_memory WHERE project_id NOT IN (SELECT id FROM project);
DELETE FROM job WHERE project_id NOT IN (SELECT id FROM project);
