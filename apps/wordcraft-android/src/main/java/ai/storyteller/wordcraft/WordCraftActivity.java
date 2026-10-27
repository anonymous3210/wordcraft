package ai.storyteller.wordcraft;

import android.app.AlertDialog;
import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;

/** Minimal Java bridge for the Android Storage Access Framework. */
public final class WordCraftActivity extends NativeActivity {
    private static final String TAG = "WordCraftSAF";
    // NativeActivity loads the app library for android_main; System.loadLibrary also associates
    // it with this Java class loader so nativeOnSafResult can resolve its exported JNI symbol.
    static {
        System.loadLibrary("wordcraft_android");
    }

    private static final int REQUEST_CODE_BASE = 0x5A00;
    private static final int REQUEST_CODE_END = 0x5AFF;
    private static final long MAX_DOCUMENT_BYTES = 2L * 1024L * 1024L * 1024L;
    private static final int COPY_BUFFER_BYTES = 64 * 1024;

    private File traceFile() {
        File directory = new File(getFilesDir(), "wordcraft-saf");
        if (!directory.exists()) directory.mkdirs();
        return new File(directory, "saf-diagnostic.log");
    }

    private void trace(String message) {
        String line = System.currentTimeMillis() + " [JAVA] " + message + "\n";
        try (FileOutputStream output = new FileOutputStream(traceFile(), true)) {
            output.write(line.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            output.getFD().sync();
        } catch (Throwable error) {
            Log.e(TAG, "Could not persist diagnostic trace", error);
        }
        Log.i(TAG, message);
    }

    private void showPreviousTrace() {
        File file = traceFile();
        if (!file.isFile() || file.length() == 0) return;
        try {
            int length = (int) Math.min(file.length(), 48 * 1024);
            byte[] bytes = new byte[length];
            try (FileInputStream input = new FileInputStream(file)) {
                long toSkip = Math.max(0, file.length() - length);
                while (toSkip > 0) {
                    long skipped = input.skip(toSkip);
                    if (skipped <= 0) break;
                    toSkip -= skipped;
                }
                int offset = 0;
                while (offset < bytes.length) {
                    int count = input.read(bytes, offset, bytes.length - offset);
                    if (count < 0) break;
                    offset += count;
                }
                if (offset != bytes.length) bytes = java.util.Arrays.copyOf(bytes, offset);
            }
            android.widget.TextView view = new android.widget.TextView(this);
            view.setText(new String(bytes, java.nio.charset.StandardCharsets.UTF_8));
            view.setTextIsSelectable(true);
            view.setPadding(32, 24, 32, 24);
            android.widget.ScrollView scroll = new android.widget.ScrollView(this);
            scroll.addView(view);
            new AlertDialog.Builder(this)
                    .setTitle("WordCraft SAF diagnostic log")
                    .setView(scroll)
                    .setPositiveButton("Close", null)
                    .setNeutralButton("Clear log", (dialog, which) -> {
                        try (FileOutputStream output = new FileOutputStream(file, false)) {
                            output.getFD().sync();
                        } catch (IOException error) {
                            trace("TRACE_CLEAR_FAILED " + error);
                        }
                    })
                    .show();
        } catch (Throwable error) {
            Log.e(TAG, "Could not display previous diagnostic trace", error);
        }
    }

    @Override
    protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        trace("ACTIVITY onCreate");
        showPreviousTrace();
    }

    private static String stackTrace(Throwable error) {
        StringWriter writer = new StringWriter();
        error.printStackTrace(new PrintWriter(writer));
        return writer.toString();
    }

    private native void nativeOnSafResult(
            int requestCode,
            int resultCode,
            String[] backingPaths,
            String[] documentUris,
            String errorMessage);

    /** Opens a user-selected document or documents through ACTION_OPEN_DOCUMENT. */
    public void openSafPicker(int requestCode, String purpose, boolean allowMultiple) {
        trace("PICKER_OPEN begin request=" + requestCode + " multiple=" + allowMultiple + " purpose=" + purpose);
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        String[] mimeTypes = mimeTypesForPurpose(purpose);
        if (mimeTypes.length > 0) {
            intent.putExtra(Intent.EXTRA_MIME_TYPES, mimeTypes);
        }
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        startActivityForResult(intent, requestCode);
        trace("PICKER_OPEN intent returned request=" + requestCode);
    }

    /** Creates a destination document through ACTION_CREATE_DOCUMENT. */
    public void createSafPicker(int requestCode, String suggestedName) {
        trace("PICKER_CREATE begin request=" + requestCode + " name=" + suggestedName);
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType(mimeTypeForName(suggestedName));
        intent.putExtra(Intent.EXTRA_TITLE, suggestedName);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        startActivityForResult(intent, requestCode);
        trace("PICKER_CREATE intent returned request=" + requestCode);
    }

    /** Copies a staged private file to the selected content URI through ContentResolver. */
    public String writeSafDocument(String uriText, String sourcePath) {
        trace("WRITE entry uri=" + uriText + " source=" + sourcePath);
        Uri uri = Uri.parse(uriText);
        File source = new File(sourcePath);
        trace("WRITE opening streams sourceExists=" + source.exists() + " sourceBytes=" + source.length());
        try (InputStream input = new FileInputStream(source);
             OutputStream output = getContentResolver().openOutputStream(uri, "rwt")) {
            if (output == null) {
                trace("WRITE provider returned null OutputStream");
                return "The document provider did not open the selected file for writing.";
            }
            byte[] buffer = new byte[COPY_BUFFER_BYTES];
            long total = 0;
            int count;
            while ((count = input.read(buffer)) != -1) {
                total += count;
                if (total > MAX_DOCUMENT_BYTES) {
                    return "The document is larger than the 2 GiB SAF transfer limit.";
                }
                output.write(buffer, 0, count);
            }
            trace("WRITE bytes copied=" + total + "; flushing");
            output.flush();
            trace("WRITE completed bytes=" + total);
            return null;
        } catch (Exception error) {
            Log.e(TAG, "Document write failed: " + error.getClass().getSimpleName(), error);
            trace("WRITE exception " + stackTrace(error));
            String message = error.getMessage();
            return message == null ? error.getClass().getSimpleName() : message;
        } catch (Error error) {
            trace("WRITE fatal Error " + stackTrace(error));
            throw error;
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode >= REQUEST_CODE_BASE && requestCode <= REQUEST_CODE_END) {
            trace("RESULT intercepted request=" + requestCode + " result=" + resultCode + " dataNull=" + (data == null));
            Intent resultData = data;
            trace("RESULT starting worker request=" + requestCode);
            new Thread(
                    () -> {
                        try {
                            processSafResult(requestCode, resultCode, resultData);
                        } catch (Throwable error) {
                            trace("RESULT uncaught throwable " + stackTrace(error));
                            if (error instanceof Error) throw (Error) error;
                            if (error instanceof RuntimeException) throw (RuntimeException) error;
                            throw new RuntimeException(error);
                        }
                    },
                    "wordcraft-saf").start();
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }

    private void processSafResult(int requestCode, int resultCode, Intent data) {
        trace("RESULT worker entered request=" + requestCode + " result=" + resultCode + " dataNull=" + (data == null));
        if (resultCode != RESULT_OK || data == null) {
            trace("RESULT cancellation native callback begin");
            nativeOnSafResult(requestCode, resultCode, new String[0], new String[0], null);
            trace("RESULT cancellation native callback returned");
            return;
        }

        boolean saveRequest = (requestCode & 0x0080) != 0;
        ArrayList<File> copiedFiles = new ArrayList<>();
        try {
            List<Uri> selected = selectedUris(data, !saveRequest);
            trace("RESULT selected count=" + selected.size() + " save=" + saveRequest);
            if (selected.isEmpty()) {
                nativeOnSafResult(requestCode, RESULT_CANCELED, new String[0], new String[0], null);
                Log.i(TAG, "Empty selection callback completed");
                return;
            }

            ArrayList<String> backingPaths = new ArrayList<>();
            ArrayList<String> documentUris = new ArrayList<>();
            int grantedFlags = data.getFlags() & (Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);

            for (Uri uri : selected) {
                trace("RESULT persist permission begin uri=" + uri + " flags=" + grantedFlags);
                persistUriPermission(uri, grantedFlags, saveRequest);
                trace("RESULT persist permission returned");
                String displayName = queryDisplayName(uri);
                trace("RESULT displayName=" + displayName);
                File destination = newBackingFile(displayName);
                trace("RESULT backingPath=" + destination.getAbsolutePath());
                if (!saveRequest) {
                    copiedFiles.add(destination);
                    trace("RESULT copy from URI begin uri=" + uri);
                    copyUriToFile(uri, destination);
                    trace("RESULT copy from URI complete bytes=" + destination.length());
                }
                backingPaths.add(destination.getAbsolutePath());
                documentUris.add(uri.toString());
            }

            trace("RESULT native callback begin paths=" + backingPaths.size() + " uris=" + documentUris.size());
            nativeOnSafResult(
                    requestCode,
                    RESULT_OK,
                    backingPaths.toArray(new String[0]),
                    documentUris.toArray(new String[0]),
                    null);
            trace("RESULT native callback returned");
        } catch (Exception error) {
            trace("RESULT processing exception " + stackTrace(error));
            for (File file : copiedFiles) {
                if (file.exists()) {
                    file.delete();
                }
            }
            Log.e(TAG, "Picker result processing failed: " + error.getClass().getSimpleName(), error);
            String message = error.getMessage();
            trace("RESULT error native callback begin");
            nativeOnSafResult(
                    requestCode,
                    RESULT_OK,
                    new String[0],
                    new String[0],
                    message == null ? error.getClass().getSimpleName() : message);
            trace("RESULT error native callback returned");
        }
    }

    private List<Uri> selectedUris(Intent data, boolean allowMultiple) {
        ArrayList<Uri> selected = new ArrayList<>();
        if (allowMultiple && data.getClipData() != null) {
            int count = Math.min(data.getClipData().getItemCount(), 2000);
            for (int index = 0; index < count; index++) {
                Uri uri = data.getClipData().getItemAt(index).getUri();
                if (uri != null) {
                    selected.add(uri);
                }
            }
        } else if (data.getData() != null) {
            selected.add(data.getData());
        }
        return selected;
    }

    private File newBackingFile(String displayName) throws IOException {
        File directory = new File(new File(getFilesDir(), "wordcraft-saf"), UUID.randomUUID().toString());
        if (!directory.mkdirs() && !directory.isDirectory()) {
            throw new IOException("Could not create the private SAF working directory.");
        }
        return new File(directory, safeFileName(displayName));
    }

    private static String safeFileName(String name) {
        String candidate = name == null ? "" : name.trim();
        candidate = candidate.replaceAll("[\\p{Cntrl}/\\\\:*?\"<>|]", "_");
        if (candidate.isEmpty() || candidate.equals(".") || candidate.equals("..")) {
            candidate = "document";
        }
        if (candidate.length() > 180) {
            candidate = candidate.substring(0, 180);
        }
        return candidate;
    }

    private void persistUriPermission(Uri uri, int flags, boolean saveRequest) throws IOException {
        trace("PERMISSION check uri=" + uri + " flags=" + flags + " save=" + saveRequest);
        int requiredFlag = saveRequest ? Intent.FLAG_GRANT_WRITE_URI_PERMISSION : Intent.FLAG_GRANT_READ_URI_PERMISSION;
        if ((flags & requiredFlag) == 0) {
            throw new IOException(saveRequest
                    ? "The document provider did not grant write access to the selected destination."
                    : "The document provider did not grant read access to the selected document.");
        }
        try {
            getContentResolver().takePersistableUriPermission(uri, flags);
            trace("PERMISSION persist completed uri=" + uri);
        } catch (SecurityException | IllegalArgumentException error) {
            trace("PERMISSION persist exception " + stackTrace(error));
            throw new IOException("The document provider did not allow this URI permission to persist.", error);
        }
    }

    private String queryDisplayName(Uri uri) {
        String name = null;
        try (Cursor cursor = getContentResolver().query(
                uri,
                new String[]{OpenableColumns.DISPLAY_NAME},
                null,
                null,
                null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (index >= 0) {
                    name = cursor.getString(index);
                }
            }
        } catch (Exception ignored) {
            // Some providers omit metadata; the URI segment is a reasonable fallback.
        }
        if (name == null || name.trim().isEmpty()) {
            String segment = uri.getLastPathSegment();
            name = segment == null || segment.trim().isEmpty() ? "document" : segment;
        }
        return safeFileName(name);
    }

    private void copyUriToFile(Uri uri, File destination) throws IOException {
        trace("IMPORT openInputStream begin uri=" + uri);
        InputStream input = getContentResolver().openInputStream(uri);
        if (input == null) {
            throw new IOException("The document provider did not open the selected file for reading.");
        }
        try (InputStream source = input; OutputStream output = new FileOutputStream(destination)) {
            trace("IMPORT streams open destination=" + destination.getAbsolutePath());
            byte[] buffer = new byte[COPY_BUFFER_BYTES];
            long total = 0;
            int count;
            while ((count = source.read(buffer)) != -1) {
                total += count;
                if (total > MAX_DOCUMENT_BYTES) {
                    throw new IOException("The document is larger than the 2 GiB SAF transfer limit.");
                }
                output.write(buffer, 0, count);
            }
            output.flush();
            trace("IMPORT copied bytes=" + total);
        } catch (IOException error) {
            trace("IMPORT IOException " + stackTrace(error));
            if (destination.exists()) {
                destination.delete();
            }
            throw error;
        }
    }

    private static String[] mimeTypesForPurpose(String purpose) {
        if ("picture".equals(purpose)) {
            return new String[]{"image/*"};
        }
        if ("font".equals(purpose)) {
            return new String[]{"font/*", "application/x-font-ttf", "application/vnd.ms-opentype"};
        }
        if ("recipients".equals(purpose)) {
            return new String[]{"text/csv", "text/tab-separated-values", "text/plain",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    "application/vnd.ms-excel", "application/vnd.oasis.opendocument.spreadsheet"};
        }
        if ("sources".equals(purpose)) {
            return new String[]{"application/xml", "text/xml"};
        }
        return new String[0];
    }

    private static String mimeTypeForName(String name) {
        String lower = name == null ? "" : name.toLowerCase(java.util.Locale.ROOT);
        if (lower.endsWith(".pdf")) return "application/pdf";
        if (lower.endsWith(".png")) return "image/png";
        if (lower.endsWith(".txt")) return "text/plain";
        if (lower.endsWith(".md")) return "text/markdown";
        if (lower.endsWith(".csv")) return "text/csv";
        if (lower.endsWith(".xml")) return "application/xml";
        if (lower.endsWith(".html") || lower.endsWith(".htm")) return "text/html";
        if (lower.endsWith(".rtf")) return "application/rtf";
        if (lower.endsWith(".tex")) return "application/x-tex";
        if (lower.endsWith(".json")) return "application/json";
        if (lower.endsWith(".odt")) return "application/vnd.oasis.opendocument.text";
        if (lower.endsWith(".docm")) return "application/vnd.ms-word.document.macroEnabled.12";
        if (lower.endsWith(".dotx")) return "application/vnd.openxmlformats-officedocument.wordprocessingml.template";
        if (lower.endsWith(".dotm")) return "application/vnd.ms-word.template.macroEnabled.12";
        if (lower.endsWith(".docx")) return "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        return "application/octet-stream";
    }
}
