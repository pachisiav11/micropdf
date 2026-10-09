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
typedef struct pdf_image_rewriter_options pdf_image_rewriter_options;
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
void pdf_rewrite_images(fz_context *ctx, pdf_document *doc, pdf_image_rewriter_options *opts);

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

int mp_pdf_rewrite_images(fz_context *ctx, pdf_document *doc, pdf_image_rewriter_options *opts, const char **err)
{
	TRY(ctx) { pdf_rewrite_images(ctx, doc, opts); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* Removing one image from a page: MuPDF's sanitize filter (mupdf/pdf/interpret.h) asks its
 * culler about each image it meets, with the image's box in the page's user space. */
typedef struct { float x0, y0, x1, y1; } fz_rect;
typedef struct pdf_page pdf_page;
typedef struct pdf_processor pdf_processor;
typedef struct pdf_filter_options pdf_filter_options;
typedef pdf_processor *(pdf_filter_factory_fn)(fz_context *ctx, pdf_document *doc, pdf_processor *chain, int struct_parents, fz_matrix transform, pdf_filter_options *options, void *factory_options);
typedef struct { pdf_filter_factory_fn *filter; void *options; } pdf_filter_factory;
struct pdf_filter_options {
	int recurse;
	int instance_forms;
	int ascii;
	int no_update;
	void *opaque;
	void (*complete)(fz_context *ctx, void *buffer, void *opaque);
	pdf_filter_factory *filters;
	int newlines;
};
typedef struct {
	void *opaque;
	void *image_filter;
	void *text_filter;
	void *after_text_object;
	int (*culler)(fz_context *ctx, void *opaque, fz_rect bbox, int type);
} pdf_sanitize_filter_options;
pdf_processor *pdf_new_sanitize_filter(fz_context *ctx, pdf_document *doc, pdf_processor *chain, int struct_parents, fz_matrix transform, pdf_filter_options *options, void *sopts);
void pdf_filter_page_contents(fz_context *ctx, pdf_document *doc, pdf_page *page, pdf_filter_options *options);

/* fz_cull_type's FZ_CULL_IMAGE. */
#define CULL_IMAGE 9

typedef struct { fz_rect target; float slack; int removed; } image_cull;

static int cull_image(fz_context *ctx, void *opaque, fz_rect r, int type)
{
	image_cull *c = opaque;
	float s = c->slack;
	(void)ctx;
	if (type != CULL_IMAGE)
		return 0;
	if (r.x0 < c->target.x0 - s || r.x0 > c->target.x0 + s || r.x1 < c->target.x1 - s || r.x1 > c->target.x1 + s ||
		r.y0 < c->target.y0 - s || r.y0 > c->target.y0 + s || r.y1 < c->target.y1 - s || r.y1 > c->target.y1 + s)
		return 0;
	c->removed++;
	return 1;
}

/* Removes the images drawn over `target` (within `slack` on each side) from the page's
 * contents and the forms they draw, which are copied for this page first. */
int mp_pdf_remove_image(fz_context *ctx, pdf_document *doc, pdf_page *page, fz_rect target, float slack, int *removed, const char **err)
{
	image_cull cull = { target, slack, 0 };
	pdf_sanitize_filter_options sopts = { &cull, NULL, NULL, NULL, cull_image };
	pdf_filter_factory filters[2] = { { pdf_new_sanitize_filter, &sopts }, { NULL, NULL } };
	struct pdf_filter_options opts = { 1, 1, 0, 0, NULL, NULL, filters, 0 };
	TRY(ctx) { pdf_filter_page_contents(ctx, doc, page, &opts); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	*removed = cull.removed;
	return 0;
}
