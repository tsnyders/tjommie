
UPDATE task_evaluations SET data=json_set(data, '$.reviewCoverage', json('null'))
WHERE json_type(data, '$.reviewCoverage') IS NULL;
