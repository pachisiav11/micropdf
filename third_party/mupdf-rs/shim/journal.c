/*
 * fz_try wrappers for the parts of MuPDF's undo journal, annotations, form widgets, text and
 * font subsetting that mupdf-sys does not wrap (it does wrap pdf_begin_operation,
 * pdf_end_operation and pdf_abandon_operation).
 *
 * mupdf-sys does not export its include directory, so the few declarations needed are
 * repeated here. They match MuPDF 1.27 (mupdf/fitz/context.h, mupdf/fitz/system.h,
 * mupdf/fitz/geometry.h, mupdf/fitz/text.h, mupdf/pdf/object.h, mupdf/pdf/annot.h,
 * mupdf/pdf/form.h and mupdf/pdf/font.h). On Windows MuPDF uses plain setjmp/longjmp
 * (HAVE_SIGSETJMP is 0).
 */
#include <setjmp.h>

typedef struct fz_context fz_context;
typedef struct pdf_document pdf_document;
typedef struct pdf_annot pdf_annot;
typedef struct fz_display_list fz_display_list;
typedef struct fz_text fz_text;
typedef struct fz_font fz_font;
typedef struct { float a, b, c, d, e, f; } fz_matrix;

extern const fz_matrix fz_identity;

jmp_buf *fz_push_try(fz_context *ctx);
int fz_do_try(fz_context *ctx);
int fz_do_catch(fz_context *ctx);
const char *fz_caught_message(fz_context *ctx);

void pdf_enable_journal(fz_context *ctx, pdf_document *doc);
int pdf_undoredo_state(fz_context *ctx, pdf_document *doc, int *steps);
const char *pdf_undoredo_step(fz_context *ctx, pdf_document *doc, int step);
void pdf_undo(fz_context *ctx, pdf_document *doc);
void pdf_redo(fz_context *ctx, pdf_document *doc);
int pdf_toggle_widget(fz_context *ctx, pdf_annot *widget);
int pdf_choice_widget_options(fz_context *ctx, pdf_annot *tw, int exportval, const char *opts[]);
void pdf_set_annot_appearance_from_display_list(fz_context *ctx, pdf_annot *annot, const char *appearance, const char *state, fz_matrix ctm, fz_display_list *list);
/* The last two parameters are the enums fz_bidi_direction and fz_text_language. */
void fz_show_glyph(fz_context *ctx, fz_text *text, fz_font *font, fz_matrix trm, int glyph, int unicode, int wmode, int bidi_level, int markup_dir, int language);
void pdf_subset_fonts(fz_context *ctx, pdf_document *doc, int pages_len, const int *pages);

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

int mp_pdf_toggle_widget(fz_context *ctx, pdf_annot *widget, int *toggled, const char **err)
{
	TRY(ctx) { *toggled = pdf_toggle_widget(ctx, widget); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* With opts NULL, only counts the options. The strings belong to the document. */
int mp_pdf_choice_widget_options(fz_context *ctx, pdf_annot *widget, const char **opts, int *count, const char **err)
{
	TRY(ctx) { *count = pdf_choice_widget_options(ctx, widget, 0, opts); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* Sets the normal appearance to the display list's drawing, which MuPDF fits to the Rect. */
int mp_pdf_set_annot_appearance(fz_context *ctx, pdf_annot *annot, fz_display_list *list, const char **err)
{
	TRY(ctx) { pdf_set_annot_appearance_from_display_list(ctx, annot, "N", NULL, fz_identity, list); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* Adds one glyph, set horizontally left to right (FZ_BIDI_LTR), language unset. */
int mp_show_glyph(fz_context *ctx, fz_text *text, fz_font *font, fz_matrix trm, int glyph, int unicode, const char **err)
{
	TRY(ctx) { fz_show_glyph(ctx, text, font, trm, glyph, unicode, 0, 0, 0, 0); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* Cuts every embedded font down to the glyphs the document's pages use. */
int mp_pdf_subset_fonts(fz_context *ctx, pdf_document *doc, const char **err)
{
	TRY(ctx) { pdf_subset_fonts(ctx, doc, 0, NULL); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}
