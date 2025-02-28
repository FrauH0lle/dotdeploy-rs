;;; /home/roland/Projekte/dotdeploy-rewrite/src/modules/bla.el -- DESCRIPTION -*- lexical-binding: t -*-

signal(excessive-lisp-nesting (1601))

(condition-case err
    (let* ((checker (flycheck-get-checker-for-buffer)))
      (if checker
          (flycheck-start-current-syntax-check checker)
        (flycheck-clear)
        (flycheck-report-status 'no-checker)))
  (error (flycheck-report-failed-syntax-check)
         (signal (car err) (cdr err))))

(if (flycheck-running-p)
    nil
  (run-hooks 'flycheck-before-syntax-check-hook)
  (flycheck-clear-errors)
  (flycheck-mark-all-overlays-for-deletion)
  (condition-case err
      (let* ((checker (flycheck-get-checker-for-buffer)))
        (if checker
            (flycheck-start-current-syntax-check checker)
          (flycheck-clear)
          (flycheck-report-status 'no-checker)))
    (error (flycheck-report-failed-syntax-check) (signal (car err) (cdr err)))))

(if flycheck-mode
    (if (flycheck-running-p)
        nil
      (run-hooks 'flycheck-before-syntax-check-hook)
      (flycheck-clear-errors)
      (flycheck-mark-all-overlays-for-deletion)
      (condition-case err
          (let* ((checker (flycheck-get-checker-for-buffer)))
            (if checker
                (flycheck-start-current-syntax-check checker)
              (flycheck-clear)
              (flycheck-report-status 'no-checker)))
        (error (flycheck-report-failed-syntax-check)
               (signal (car err) (cdr err)))))
  (user-error "Flycheck mode disabled"))

flycheck-buffer()


(provide 'bla)
;;; bla.el ends here
