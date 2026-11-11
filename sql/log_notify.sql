-- Функция логирования через NOTIFY для PostgreSQL 14+.
--
-- Использование внутри любой функции/процедуры/триггера:
--     PERFORM log_notify('my_channel', 'обработано ' || n || ' строк');
-- В канал my_channel уйдёт сообщение вида:
--     [имя_вызвавшей_функции] обработано 10 строк
--
-- Имя вызывающей функции берётся из стека вызовов (GET DIAGNOSTICS ... PG_CONTEXT).
-- Если функция вызвана не из функции (из psql, анонимного блока DO), вместо имени
-- подставляется SQL или DO соответственно.
--
-- Важно: NOTIFY транзакционный. Сообщения доставляются слушателям только при COMMIT,
-- при ROLLBACK они теряются, а одинаковые сообщения в одной транзакции схлопываются в одно.

CREATE OR REPLACE FUNCTION public.log_notify(p_channel text, p_message text)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    v_stack   text;
    v_line    text;
    v_caller  text;
    v_payload text;
BEGIN
    GET DIAGNOSTICS v_stack = PG_CONTEXT;
    -- Стек вызовов, по строке на уровень, начиная с самой log_notify:
    --   PL/pgSQL function log_notify(text,text) line 8 at GET DIAGNOSTICS
    --   SQL statement "SELECT log_notify('ch', 'msg')"
    --   PL/pgSQL function app.process_orders(integer) line 3 at PERFORM
    -- Ищем первую функцию после log_notify.
    FOREACH v_line IN ARRAY (string_to_array(v_stack, E'\n'))[2:] LOOP
        IF v_line LIKE 'PL/pgSQL function inline_code_block%' THEN
            v_caller := 'DO';
        ELSIF v_line LIKE 'PL/pgSQL function %' OR v_line LIKE 'SQL function %' THEN
            v_caller := coalesce(
                substring(v_line FROM '^(?:PL/pgSQL|SQL) function "([^"]+)"'),
                substring(v_line FROM '^(?:PL/pgSQL|SQL) function (.+?)\('),
                substring(v_line FROM '^(?:PL/pgSQL|SQL) function (\S+)'));
        END IF;
        EXIT WHEN v_caller IS NOT NULL;
    END LOOP;
    v_caller := coalesce(v_caller, 'SQL');

    v_payload := '[' || v_caller || '] ' || coalesce(p_message, '<NULL>');

    -- Payload NOTIFY ограничен 8000 байт: обрезаем по символам, чтобы не разрезать UTF-8.
    IF octet_length(v_payload) > 7999 THEN
        v_payload := left(v_payload, 7996);
        WHILE octet_length(v_payload) > 7996 LOOP
            v_payload := left(v_payload,
                length(v_payload) - greatest((octet_length(v_payload) - 7996) / 4, 1));
        END LOOP;
        v_payload := v_payload || '…';
    END IF;

    PERFORM pg_notify(p_channel, v_payload);
END;
$$;
