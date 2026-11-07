WITH attachments AS (
 SELECT COALESCE(sum(COALESCE(file_file_size::bigint,0)+COALESCE(thumbnail_file_size::bigint,0)) FILTER (WHERE COALESCE(remote_url,'')=''),0) local,
 COALESCE(sum(COALESCE(file_file_size::bigint,0)+COALESCE(thumbnail_file_size::bigint,0)) FILTER (WHERE COALESCE(remote_url,'')<>''),0) remote FROM media_attachments
), profiles AS (
 SELECT COALESCE(sum(COALESCE(avatar_file_size::bigint,0)+COALESCE(header_file_size::bigint,0)) FILTER (WHERE domain IS NULL),0) local,
 COALESCE(sum(COALESCE(avatar_file_size::bigint,0)+COALESCE(header_file_size::bigint,0)) FILTER (WHERE domain IS NOT NULL),0) remote FROM accounts
), emoji AS (
 SELECT COALESCE(sum(image_file_size::bigint) FILTER (WHERE domain IS NULL),0) local,
 COALESCE(sum(image_file_size::bigint) FILTER (WHERE domain IS NOT NULL),0) remote FROM custom_emojis
) SELECT (a.local+p.local+e.local)::bigint,(a.remote+p.remote+e.remote)::bigint
FROM attachments a,profiles p,emoji e
