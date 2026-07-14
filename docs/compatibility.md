## Execution command result behavior

QNX GDB 6.8 can produce more than one result record with the same token.

Example without a loaded inferior:

```text
1-exec-next
1^running
(gdb)
&"The program is not being run.\n"
1^error,msg="The program is not being run."
(gdb)
```

Therefore, the first `^running` result must not always be treated as the final
successful outcome of an execution command.

Non-execution commands can use the synchronous command/result API.

Execution commands require a separate asynchronous state machine capable of
processing:

- the initial `^running` record;
- later stream output;
- a later `^error` record with the same token;
- `*running` and `*stopped` asynchronous records.
