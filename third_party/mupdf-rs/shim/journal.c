/*
 * fz_try wrappers for the parts of MuPDF's undo journal that mupdf-sys does not wrap
 * (it does wrap pdf_begin_operation, pdf_end_operation and pdf_abandon_operation).
 *
 * mupdf-sys does not export its include directory, so the few declarations needed are
 * repeated here. They match MuPDF 1.27 (mupdf/fitz/context.h, mupdf/fitz/system.h and
 * mupdf/pdf/object.h). On Windows MuPDF uses plain setjmp/longjmp (HAVE_SIGSETJMP is 0).
 */
#include <setjmp.h>

typedef struct fz_context fz_context;
typedef struct pdf_document pdf_document;

jmp_buf *fz_push_try(fz_context *ctx);
int fz_do_try(fz_context *ctx);
int fz_do_catch(fz_context *ctx);
const char *fz_caught_message(fz_context *ctx);

void pdf_enable_journal(fz_context *ctx, pdf_document *doc);
int pdf_undoredo_state(fz_context *ctx, pdf_document *doc, int *steps);
const char *pdf_undoredo_step(fz_context *ctx, pdf_document *doc, int step);
void pdf_undo(fz_context *ctx, pdf_document *doc);
void pdf_redo(fz_context *ctx, pdf_document *doc);

/* The expansion of MuPDF's fz_try / fz_catch macros. */
#define TRY(ctx) if (!setjmp(*fz_push_try(ctx))) if (fz_do_try(ctx)) do
#define CATCH(ctx) while (0); if (fz_do_catch(ctx))

/* Each wrapper returns 0, or -1 with *err pointing at the context's error message, which
 * stays valid until the next MuPDF error on that context. */

int mp_pdf_enable_journal(fz_context *ctx, pdf_document *doc, const char **err)
{
	TRY(ctx) { pdf_enable_journal(ctx, doc); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

int mp_pdf_undoredo_state(fz_context *ctx, pdf_document *doc, int *current, int *steps, const char **err)
{
	TRY(ctx) { *current = pdf_undoredo_state(ctx, doc, steps); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

int mp_pdf_undoredo_step(fz_context *ctx, pdf_document *doc, int step, const char **name, const char **err)
{
	TRY(ctx) { *name = pdf_undoredo_step(ctx, doc, step); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

int mp_pdf_undo(fz_context *ctx, pdf_document *doc, const char **err)
{
	TRY(ctx) { pdf_undo(ctx, doc); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

int mp_pdf_redo(fz_context *ctx, pdf_document *doc, const char **err)
{
	TRY(ctx) { pdf_redo(ctx, doc); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}
