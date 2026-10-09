;;; NOERROR only suppresses unavailable files, not evaluation failures.
(when (boundp 'neo-r019-observed-coding)
  (setq neo-r019-observed-coding coding-system-for-read))
(string-match "x" "x")
(error "neo-r019-genuine-evaluation-error")
