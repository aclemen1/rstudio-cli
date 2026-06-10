/*
 * browse_level.c — count active R browser contexts (the N of "Browse[N]>").
 *
 * Compiled lazily at runtime by R/native.R (rscli_browse_level), never at
 * package-install time, so the rstudiocli package stays installable on hosts
 * without a C toolchain. See R/native.R for the compile/cache/load machinery.
 *
 * Why this exists: R's "Browse[N]>" level is the count of CTXT_BROWSER
 * contexts on the interpreter's context stack. R does NOT expose it to the R
 * language (browser() is a .Primitive; sys.calls()/sys.nframe() don't reflect
 * it), and rsession regex-matches the prompt to a boolean and discards the
 * digits. The only way to recover N is to walk R's context stack in C, which
 * is exactly what R's own internal Rf_countContexts(CTXT_BROWSER, TRUE) does —
 * but that symbol is attribute_hidden (not linkable). So we replicate the
 * minimal walk here.
 *
 * Stability / safety notes:
 *  - R_GlobalContext is exported by libR as an opaque `void *`
 *    (Rinterface.h). We cast it to a minimal struct exposing only the first
 *    two RCNTXT fields — `nextcontext` (offset 0) and `callflag` (offset 8 on
 *    LP64) — whose layout has been stable across modern R releases. We never
 *    touch deeper fields, so RCNTXT layout churn elsewhere cannot affect us.
 *  - R_ToplevelContext (R's loop terminator) is NOT exported, so we terminate
 *    on a NULL `nextcontext` instead. The toplevel context sits at the bottom
 *    of the chain with a NULL successor, so this counts every CTXT_BROWSER on
 *    the stack — exactly the browse level.
 *  - CTXT_BROWSER == 16 has been constant for a very long time.
 *  - A step guard caps the walk so a corrupt/unexpected chain can never spin.
 *  - Single-threaded by construction: only ever called at an idle R prompt
 *    (including a Browse prompt) via execute_r_code.
 */

#include <R.h>
#include <Rinternals.h>

/* Exported by libR as an opaque pointer (see R's Rinterface.h). */
extern void *R_GlobalContext;

/* Minimal prefix of RCNTXT: only the two leading fields we rely on. */
typedef struct RSCLI_MiniCtx {
    struct RSCLI_MiniCtx *nextcontext;
    int callflag;
} RSCLI_MiniCtx;

#define RSCLI_CTXT_BROWSER 16
#define RSCLI_MAX_STEPS 100000

SEXP rscli_browse_level(void) {
    int n = 0, steps = 0;
    RSCLI_MiniCtx *c = (RSCLI_MiniCtx *) R_GlobalContext;
    while (c != NULL && steps < RSCLI_MAX_STEPS) {
        if (c->callflag == RSCLI_CTXT_BROWSER)
            n++;
        c = c->nextcontext;
        steps++;
    }
    return ScalarInteger(n);
}
