// The `frust-native-widgets` plugin's Android app-chrome host: Material 3 bars
// (AppBarLayout + MaterialToolbar on top, BottomNavigationView at the bottom)
// around the frust embedding's content root, driven from Rust's `ChromeHost`
// (`plugins/native-widgets/src/chrome/host.rs`) over JNI.
//
// FROZEN once shipped: the binary class name `dev.frust.nativewidgets.FrustNativeChrome`
// (Rust loads it through the application classloader), the `@JvmStatic` entry
// points `install`/`apply`/`dispose`/`setIcon`/`setScroll`, the JNI exports
// `Java_dev_frust_nativewidgets_FrustNativeChrome_nativeOnChromeEvent` and
// `..._nativeOnChromeLost`, the `INSTALL_*`/`ICON_SLOT_*` codes, the JSON keys
// below and [CHROME_MENU_STRIDE]/[CHROME_TAB_RESELECT_SHIFT] (twins of
// `src/events.rs`). Event kinds are `FrustNativeListener`'s `KIND_CHROME_*`.
//
// Flat JSON keys (`install` takes all of them, `apply` only the fields its
// `changed` array names — `ChromeField` spellings):
//   Tabs:     tabCount, tab<i>Title, tab<i>Enabled, tab<i>Badge (absent/null =
//             none, "" = dot), tab<i>IconRev (0 = no icon)
//   Selected: selectedIndex (-1 = none)
//   TopBar:   hasTop          Title: title          Subtitle: subtitle (absent/null = none)
//   Leading:  leading = "none" | "back" | "image"   LargeTitle: largeTitle (ignored here)
//   Actions:  actionCount, action<i>Title, action<i>HasIcon, action<i>MenuCount,
//             menu<i>_<j>Title, menu<i>_<j>Destructive, menu<i>_<j>Enabled
//   Brightness: dark
// Icon bytes never travel as JSON: `setIcon(slot, index, png)` caches one
// bitmap per (slot, index) until `dispose`, before or after the call naming it.
package dev.frust.nativewidgets

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.content.res.ColorStateList
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Rect
import android.graphics.drawable.BitmapDrawable
import android.os.Build
import android.os.Looper
import android.text.SpannableString
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.util.Log
import android.view.ContextThemeWrapper
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.view.ViewTreeObserver
import android.widget.FrameLayout
import android.widget.PopupMenu
import androidx.core.graphics.Insets
import androidx.core.graphics.drawable.RoundedBitmapDrawableFactory
import androidx.core.view.DisplayCutoutCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.appbar.AppBarLayout
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.bottomnavigation.BottomNavigationView
import com.google.android.material.color.MaterialColors
import com.google.android.material.navigation.NavigationBarView
import dev.frust.FrustSurfaceView
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Installs Material 3 app chrome around the frust content root of the host
 * Activity ([FrustNativePresenter.hostActivity]).
 *
 * ## Layout without re-parenting
 *
 * The embedding's root `FrameLayout` (which hosts the [FrustSurfaceView]) is
 * never detached: detaching would destroy the surface synchronously, re-entering
 * the frust runtime from inside the Rust call that is installing the chrome,
 * and show a black frame. Instead the bars join `android.R.id.content` as
 * siblings placed *before* the root (so a pre-API-30 sequential insets dispatch
 * reaches them with the real insets), and the root's padding shrinks the
 * surface to the space between them — synced from the bars' laid-out heights in
 * a pre-draw pass, which cancels that one draw instead of showing a stale one.
 *
 * ## Insets
 *
 * The app bar pads itself by the status-bar/cutout insets; the bottom bar keeps
 * Material's own padding. The root gets a copy with the top/bottom system-bar
 * and cutout insets zeroed on the sides a visible bar covers, and the IME inset
 * reduced by the bottom bar's height, so the surface reports only what it
 * touches. Nothing here consumes insets.
 *
 * ## Threading and ordering
 *
 * Every entry point and listener runs on the main thread (the frust UI
 * thread), in call order; nothing is posted. Taps report synchronously with
 * the generation [install] was given. [dispose] never calls into Rust.
 */
object FrustNativeChrome {
    /** [install] built the chrome. */
    const val INSTALL_OK = 0

    /** [install] declined (non-Material theme, unexpected content root, bad spec). */
    const val INSTALL_REFUSED = 1

    /** [install] found no live Activity content to install into; retried later. */
    const val INSTALL_NO_ACTIVITY = 2

    /** A menu choice's `detail` is `action * CHROME_MENU_STRIDE + item`. */
    const val CHROME_MENU_STRIDE = 1000

    /** A tab report's reselect flag rides this bit of `detail`. */
    const val CHROME_TAB_RESELECT_SHIFT = 32

    /** Scroll offset (logical px) above which the app bar counts as lifted. */
    private const val LIFT_THRESHOLD_PX = 1f

    /** [setIcon] slot: a bottom tab's icon, `index` = tab index. */
    const val ICON_SLOT_TAB = 0

    /** [setIcon] slot: a top-bar action's icon, `index` = action index. */
    const val ICON_SLOT_ACTION = 1

    /** [setIcon] slot: the leading image (avatar), `index` ignored. */
    const val ICON_SLOT_LEADING = 2

    private const val TAG = "frust"
    private const val LEADING_NONE = "none"
    private const val LEADING_BACK = "back"
    private const val LEADING_IMAGE = "image"

    /** Material's toolbar action icon box (Material 3 spec: 24dp). */
    private const val ACTION_ICON_DP = 24

    /** The leading avatar's diameter (Material 3 top app bar avatar: 32dp). */
    private const val AVATAR_DP = 32

    private class Tab(val title: String, val enabled: Boolean, val badge: String?, val iconRev: Long)

    private class MenuEntry(val title: String, val destructive: Boolean, val enabled: Boolean)

    private class Action(val title: String, val hasIcon: Boolean, val menu: List<MenuEntry>)

    /** The retained spec, merged field by field by [apply]. */
    private class Model {
        var tabs: List<Tab> = emptyList()
        var selected = -1
        var hasTop = false
        var title = ""
        var subtitle: String? = null
        var leading = LEADING_NONE
        var actions: List<Action> = emptyList()
        var dark = false
    }

    private class Bars(
        val context: Context,
        val appBar: AppBarLayout,
        val toolbar: MaterialToolbar,
        val bottom: BottomNavigationView,
    )

    private class Installed(
        val generation: Long,
        val activity: Activity,
        val content: FrameLayout,
        val root: ViewGroup,
        val model: Model,
        var bars: Bars,
    ) {
        val basePadding = intArrayOf(root.paddingLeft, root.paddingTop, root.paddingRight, root.paddingBottom)
        val baseRootId = root.id
        var observer: ViewTreeObserver? = null

        /** The lifted state last handed to [AppBarLayout.setLifted]; a rebuilt bar re-adopts it. */
        var lifted = false
    }

    private var live: Installed? = null
    private val icons = HashMap<Long, Bitmap>()
    private var applyingSelection = false
    private var popup: PopupMenu? = null
    private var debuggable = false

    /**
     * Build the chrome for `specJson` (every key) into the host Activity and
     * stamp every later report with `generation`. Returns an `INSTALL_*` code.
     * A chrome already installed is torn down first; cached icons are kept.
     */
    @JvmStatic
    fun install(generation: Long, specJson: String): Int {
        assertMainThread()
        live?.let {
            live = null
            detachQuietly(it)
        }
        val activity = FrustNativePresenter.hostActivity() ?: return INSTALL_NO_ACTIVITY
        debuggable = (activity.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0
        val content = activity.findViewById<ViewGroup?>(android.R.id.content) ?: return INSTALL_NO_ACTIVITY
        if (content.childCount == 0) return INSTALL_NO_ACTIVITY
        val root = content.getChildAt(0) as? ViewGroup
        if (content !is FrameLayout || root == null || !hostsSurface(root)) {
            Log.w(TAG, "Frust: native app chrome refused — the content view is not the frust embedding's root")
            return INSTALL_REFUSED
        }
        val model = Model()
        try {
            mergeAll(model, JSONObject(specJson))
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome refused — malformed spec ($t)")
            return INSTALL_REFUSED
        }
        if (model.actions.any { it.menu.size >= CHROME_MENU_STRIDE }) {
            Log.w(TAG, "Frust: native app chrome refused — a menu has $CHROME_MENU_STRIDE or more items")
            return INSTALL_REFUSED
        }
        val bars = try {
            buildBars(activity, model.dark)
        } catch (t: Throwable) {
            warnTheme(t)
            return INSTALL_REFUSED
        }
        val installed = Installed(generation, activity, content, root, model, bars)
        live = installed
        try {
            attach(installed)
            applyAll(installed)
        } catch (t: Throwable) {
            live = null
            detachQuietly(installed)
            warnTheme(t)
            return INSTALL_REFUSED
        }
        return INSTALL_OK
    }

    /** Merge the fields `diffJson`'s `changed` array names and update only those. */
    @JvmStatic
    fun apply(diffJson: String) {
        assertMainThread()
        val installed = live ?: return
        try {
            val json = JSONObject(diffJson)
            val changed = changedFields(json)
            val model = installed.model
            merge(model, json, changed)
            if ("Brightness" in changed) {
                rebuildBars(installed)
                return
            }
            if ("TopBar" in changed) applyTopVisibility(installed)
            if ("Title" in changed || "Subtitle" in changed) applyTitles(installed)
            if ("Leading" in changed) applyLeading(installed)
            if ("Actions" in changed) applyActions(installed)
            if ("Tabs" in changed) applyTabs(installed) else if ("Selected" in changed) applySelected(installed)
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome apply failed", t)
        }
    }

    /**
     * Remove the chrome and restore the content root. Idempotent and
     * panic-free; tolerates a destroyed Activity; never calls into Rust.
     */
    @JvmStatic
    fun dispose() {
        val installed = live
        live = null
        icons.clear()
        if (installed != null) detachQuietly(installed)
    }

    /** Cache icon `png` for (`slot`, `index`) and refresh the live bar showing it; empty bytes clear it. */
    @JvmStatic
    fun setIcon(slot: Int, index: Int, png: ByteArray) {
        assertMainThread()
        val key = iconKey(slot, index)
        val bitmap = if (png.isEmpty()) null else BitmapFactory.decodeByteArray(png, 0, png.size)
        if (bitmap == null) icons.remove(key) else icons[key] = bitmap
        val installed = live ?: return
        try {
            when (slot) {
                ICON_SLOT_TAB -> refreshTabIcon(installed, index)
                ICON_SLOT_ACTION -> applyActions(installed)
                ICON_SLOT_LEADING -> applyLeading(installed)
            }
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome icon refresh failed", t)
        }
    }

    /**
     * Mirror the body's scroll position: the app bar lifts once scrolled.
     *
     * The lift source is the frust scroll offset (`ChromeSpec.scroll`), NOT a
     * RecyclerView or other NestedScrollingChild: the frust surface is none,
     * so Material's own lift-on-scroll never fires and the lifted state is
     * driven by hand. Android is the only consumer of `ChromeSpec.scroll`
     * (iOS draws a frust-side edge fade instead). `(0, 0)` means at the top.
     * Main thread only; `setLifted` runs only when the state flips, so a
     * scrolling frame costs one compare.
     */
    @JvmStatic
    fun setScroll(offset: Float, max: Float) {
        val installed = live ?: return
        val lifted = offset > LIFT_THRESHOLD_PX && max > 0f
        if (lifted == installed.lifted) return
        installed.lifted = lifted
        installed.bars.appBar.setLifted(lifted)
    }

    /** [FrustNativePresenter]'s lifecycle hook: the host Activity is going away. */
    internal fun onActivityDestroyed(activity: Activity) {
        val installed = live ?: return
        if (installed.activity === activity) reportLost(installed)
    }

    @JvmStatic
    private external fun nativeOnChromeEvent(generation: Long, kind: Int, detail: Long)

    @JvmStatic
    private external fun nativeOnChromeLost(generation: Long)

    // --- spec parsing -------------------------------------------------------

    private fun mergeAll(model: Model, json: JSONObject) =
        merge(model, json, setOf("Tabs", "Selected", "TopBar", "Title", "Subtitle", "Leading", "Actions", "Brightness"))

    private fun changedFields(json: JSONObject): Set<String> {
        val array = json.optJSONArray("changed") ?: return emptySet()
        return (0 until array.length()).map { array.optString(it) }.toSet()
    }

    private fun merge(model: Model, json: JSONObject, fields: Set<String>) {
        if ("Tabs" in fields) {
            model.tabs = List(json.optInt("tabCount", 0)) { i ->
                Tab(
                    json.optString("tab${i}Title"),
                    json.optBoolean("tab${i}Enabled", true),
                    optStringOrNull(json, "tab${i}Badge"),
                    json.optLong("tab${i}IconRev", 0L),
                )
            }
        }
        if ("Selected" in fields) model.selected = json.optInt("selectedIndex", -1)
        if ("TopBar" in fields) model.hasTop = json.optBoolean("hasTop", false)
        if ("Title" in fields) model.title = json.optString("title")
        if ("Subtitle" in fields) model.subtitle = optStringOrNull(json, "subtitle")
        if ("Leading" in fields) model.leading = json.optString("leading", LEADING_NONE)
        if ("Actions" in fields) {
            model.actions = List(json.optInt("actionCount", 0)) { a ->
                val count = json.optInt("action${a}MenuCount", 0).coerceIn(0, CHROME_MENU_STRIDE - 1)
                Action(
                    json.optString("action${a}Title"),
                    json.optBoolean("action${a}HasIcon", false),
                    List(count) { m ->
                        MenuEntry(
                            json.optString("menu${a}_${m}Title"),
                            json.optBoolean("menu${a}_${m}Destructive", false),
                            json.optBoolean("menu${a}_${m}Enabled", true),
                        )
                    },
                )
            }
        }
        if ("Brightness" in fields) model.dark = json.optBoolean("dark", false)
    }

    private fun optStringOrNull(json: JSONObject, key: String): String? =
        if (!json.has(key) || json.isNull(key)) null else json.optString(key)

    // --- building and attaching --------------------------------------------

    private fun hostsSurface(root: ViewGroup): Boolean =
        (0 until root.childCount).any { root.getChildAt(it) is FrustSurfaceView }

    /**
     * Inflate the bars in the Activity's own theme with the night bit forced to
     * `dark` (the frust app's brightness, not the system's). Material's theme
     * enforcement throws here for a non-Material theme.
     */
    private fun buildBars(activity: Activity, dark: Boolean): Bars {
        val config = Configuration(activity.resources.configuration)
        val night = if (dark) Configuration.UI_MODE_NIGHT_YES else Configuration.UI_MODE_NIGHT_NO
        config.uiMode = (config.uiMode and Configuration.UI_MODE_NIGHT_MASK.inv()) or night
        val theme = activity.packageManager.getActivityInfo(activity.componentName, 0).themeResource
        val context = ContextThemeWrapper(activity, theme)
        context.applyOverrideConfiguration(config)
        val appBar = AppBarLayout(context)
        appBar.fitsSystemWindows = false
        // Lift on scroll with no liftOnScrollTargetViewId: AppBarLayout has no
        // nested-scrolling sibling to watch, so [setScroll] drives setLifted.
        // setLiftable(true) pins the liftable state so layout passes cannot
        // recompute it away (Material 1.14.0 setLiftable sets liftableOverride).
        appBar.isLiftOnScroll = true
        appBar.setLiftable(true)
        val toolbar = MaterialToolbar(context)
        appBar.addView(toolbar, AppBarLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        val bottom = BottomNavigationView(context)
        bottom.labelVisibilityMode = NavigationBarView.LABEL_VISIBILITY_LABELED
        return Bars(context, appBar, toolbar, bottom)
    }

    private fun attach(installed: Installed) {
        val (content, root) = installed.content to installed.root
        attachBars(installed)
        ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets -> rootInsets(installed, insets) }
        root.addOnAttachStateChangeListener(detachWatch)
        val observer = content.viewTreeObserver
        observer.addOnPreDrawListener(preDraw)
        installed.observer = observer
        if (root.id == View.NO_ID) root.id = View.generateViewId()
        ViewCompat.requestApplyInsets(content)
    }

    private fun attachBars(installed: Installed) {
        val bars = installed.bars
        val match = ViewGroup.LayoutParams.MATCH_PARENT
        val wrap = ViewGroup.LayoutParams.WRAP_CONTENT
        installed.content.addView(bars.appBar, 0, FrameLayout.LayoutParams(match, wrap, Gravity.TOP))
        installed.content.addView(bars.bottom, 1, FrameLayout.LayoutParams(match, wrap, Gravity.BOTTOM))
        ViewCompat.setOnApplyWindowInsetsListener(bars.appBar) { view, insets ->
            val sides = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            view.setPadding(sides.left, sides.top, sides.right, 0)
            insets
        }
        bars.bottom.addOnLayoutChangeListener(bottomHeightWatch)
        bars.bottom.setOnItemSelectedListener { item ->
            if (!applyingSelection) emitTab(item.itemId - 1, reselect = false)
            applyingSelection
        }
        bars.bottom.setOnItemReselectedListener { item ->
            if (!applyingSelection) emitTab(item.itemId - 1, reselect = true)
        }
        bars.toolbar.setOnMenuItemClickListener { item ->
            onActionClicked(item.itemId - 1)
            true
        }
        bars.bottom.accessibilityTraversalAfter = installed.root.id
    }

    private fun rebuildBars(installed: Installed) {
        val fresh = try {
            buildBars(installed.activity, installed.model.dark)
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome kept its old brightness — rebuilding the bars failed", t)
            return
        }
        dismissPopup()
        removeBars(installed)
        installed.bars = fresh
        if (installed.lifted) fresh.appBar.setLifted(true)
        attachBars(installed)
        applyAll(installed)
        ViewCompat.requestApplyInsets(installed.content)
    }

    private fun removeBars(installed: Installed) {
        val bars = installed.bars
        bars.bottom.removeOnLayoutChangeListener(bottomHeightWatch)
        installed.content.removeView(bars.appBar)
        installed.content.removeView(bars.bottom)
    }

    private fun detachQuietly(installed: Installed) {
        try {
            dismissPopup()
            releaseRoot(installed)
            removeBars(installed)
            ViewCompat.requestApplyInsets(installed.content)
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome teardown incomplete", t)
        }
    }

    /** Undo everything done to the content root; never mutates the hierarchy. */
    private fun releaseRoot(installed: Installed) {
        installed.observer?.let { if (it.isAlive) it.removeOnPreDrawListener(preDraw) }
        installed.observer = null
        val root = installed.root
        root.removeOnAttachStateChangeListener(detachWatch)
        ViewCompat.setOnApplyWindowInsetsListener(root, null)
        val base = installed.basePadding
        root.setPadding(base[0], base[1], base[2], base[3])
        root.id = installed.baseRootId
    }

    /** The Activity or its content went away under an installed chrome: report it once. */
    private fun reportLost(installed: Installed) {
        if (live !== installed) return
        live = null
        try {
            dismissPopup()
            releaseRoot(installed)
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native app chrome release after host loss incomplete", t)
        }
        nativeOnChromeLost(installed.generation)
    }

    private val detachWatch =
        object : View.OnAttachStateChangeListener {
            override fun onViewAttachedToWindow(v: View) {}

            override fun onViewDetachedFromWindow(v: View) {
                val installed = live ?: return
                if (installed.root === v) reportLost(installed)
            }
        }

    private val preDraw = ViewTreeObserver.OnPreDrawListener { syncRootPadding() }

    private val bottomHeightWatch =
        View.OnLayoutChangeListener { _, _, top, _, bottom, _, oldTop, _, oldBottom ->
            val installed = live
            if (installed != null && bottom - top != oldBottom - oldTop) {
                ViewCompat.requestApplyInsets(installed.content)
            }
        }

    /** Pad the root to the bars' laid-out heights; `false` cancels a draw that would show a stale size. */
    private fun syncRootPadding(): Boolean {
        val installed = live ?: return true
        val bars = installed.bars
        val base = installed.basePadding
        val top = base[1] + if (bars.appBar.visibility == View.VISIBLE) bars.appBar.height else 0
        val bottom = base[3] + if (bars.bottom.visibility == View.VISIBLE) bars.bottom.height else 0
        val root = installed.root
        if (root.paddingTop == top && root.paddingBottom == bottom) return true
        root.setPadding(base[0], top, base[2], bottom)
        return false
    }

    private fun rootInsets(installed: Installed, insets: WindowInsetsCompat): WindowInsetsCompat {
        val bars = installed.bars
        val topCovered = bars.appBar.visibility == View.VISIBLE
        val bottomCovered = bars.bottom.visibility == View.VISIBLE
        val builder = WindowInsetsCompat.Builder(insets)
        val sideTypes = intArrayOf(
            WindowInsetsCompat.Type.statusBars(),
            WindowInsetsCompat.Type.navigationBars(),
            WindowInsetsCompat.Type.captionBar(),
            WindowInsetsCompat.Type.displayCutout(),
        )
        for (type in sideTypes) {
            val i = insets.getInsets(type)
            builder.setInsets(type, Insets.of(i.left, if (topCovered) 0 else i.top, i.right, if (bottomCovered) 0 else i.bottom))
        }
        val cutout = insets.displayCutout
        if (cutout != null && Build.VERSION.SDK_INT >= 28) {
            val safe = Rect(
                cutout.safeInsetLeft,
                if (topCovered) 0 else cutout.safeInsetTop,
                cutout.safeInsetRight,
                if (bottomCovered) 0 else cutout.safeInsetBottom,
            )
            builder.setDisplayCutout(DisplayCutoutCompat(safe, cutout.boundingRects))
        }
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        val imeBottom = if (bottomCovered) max(0, ime.bottom - bars.bottom.height) else ime.bottom
        builder.setInsets(WindowInsetsCompat.Type.ime(), Insets.of(ime.left, ime.top, ime.right, imeBottom))
        return builder.build()
    }

    // --- applying the spec ---------------------------------------------------

    private fun applyAll(installed: Installed) {
        applyTopVisibility(installed)
        applyTitles(installed)
        applyLeading(installed)
        applyActions(installed)
        applyTabs(installed)
    }

    private fun applyTopVisibility(installed: Installed) {
        val visibility = if (installed.model.hasTop) View.VISIBLE else View.GONE
        if (installed.bars.appBar.visibility != visibility) {
            installed.bars.appBar.visibility = visibility
            ViewCompat.requestApplyInsets(installed.content)
        }
    }

    private fun applyTitles(installed: Installed) {
        installed.bars.toolbar.title = installed.model.title
        installed.bars.toolbar.subtitle = installed.model.subtitle
    }

    private fun applyLeading(installed: Installed) {
        val toolbar = installed.bars.toolbar
        val context = installed.bars.context
        when (installed.model.leading) {
            LEADING_BACK -> {
                toolbar.setNavigationIconTint(MaterialColors.getColor(toolbar, com.google.android.material.R.attr.colorOnSurface))
                toolbar.setNavigationIcon(androidx.appcompat.R.drawable.abc_ic_ab_back_material)
                toolbar.setNavigationContentDescription(androidx.appcompat.R.string.abc_action_bar_up_description)
                toolbar.setNavigationOnClickListener { emit(FrustNativeListener.KIND_CHROME_BACK, 0L) }
            }
            LEADING_IMAGE -> {
                val bitmap = icons[iconKey(ICON_SLOT_LEADING, 0)]
                toolbar.setNavigationOnClickListener(null)
                toolbar.clearNavigationIconTint()
                toolbar.navigationIcon = bitmap?.let {
                    val size = dp(context, AVATAR_DP)
                    RoundedBitmapDrawableFactory.create(context.resources, squareScaled(it, size)).apply { isCircular = true }
                }
            }
            else -> {
                toolbar.setNavigationOnClickListener(null)
                toolbar.navigationIcon = null
            }
        }
    }

    private fun applyActions(installed: Installed) {
        dismissPopup()
        val bars = installed.bars
        val menu = bars.toolbar.menu
        menu.clear()
        val tint = ColorStateList.valueOf(MaterialColors.getColor(bars.toolbar, com.google.android.material.R.attr.colorOnSurfaceVariant))
        installed.model.actions.forEachIndexed { index, action ->
            val item = menu.add(Menu.NONE, index + 1, index, action.title)
            item.setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS)
            item.contentDescription = action.title
            val bitmap = icons[iconKey(ICON_SLOT_ACTION, index)]
            if (action.hasIcon && bitmap != null) {
                item.icon = BitmapDrawable(bars.context.resources, fitScaled(bitmap, dp(bars.context, ACTION_ICON_DP)))
                item.iconTintList = tint
            }
        }
    }

    private fun onActionClicked(index: Int) {
        val installed = live ?: return
        val action = installed.model.actions.getOrNull(index) ?: return
        if (action.menu.isEmpty()) {
            emit(FrustNativeListener.KIND_CHROME_ACTION, index.toLong())
            return
        }
        val bars = installed.bars
        val anchor = bars.toolbar.findViewById<View?>(index + 1) ?: bars.toolbar
        val shown = PopupMenu(bars.context, anchor, Gravity.END)
        val danger = MaterialColors.getColor(bars.toolbar, androidx.appcompat.R.attr.colorError)
        action.menu.forEachIndexed { m, entry ->
            val title: CharSequence = if (entry.destructive) {
                SpannableString(entry.title).apply {
                    setSpan(ForegroundColorSpan(danger), 0, length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
                }
            } else {
                entry.title
            }
            shown.menu.add(Menu.NONE, m + 1, m, title).isEnabled = entry.enabled
        }
        shown.setOnMenuItemClickListener { item ->
            emit(FrustNativeListener.KIND_CHROME_MENU, index.toLong() * CHROME_MENU_STRIDE + (item.itemId - 1))
            true
        }
        shown.setOnDismissListener { if (popup === it) popup = null }
        popup = shown
        shown.show()
    }

    private fun applyTabs(installed: Installed) {
        val bottom = installed.bars.bottom
        val tabs = installed.model.tabs.take(bottom.maxItemCount)
        if (tabs.size < installed.model.tabs.size) {
            Log.w(TAG, "Frust: native app chrome shows the first ${bottom.maxItemCount} of ${installed.model.tabs.size} tabs")
        }
        applyingSelection = true
        try {
            bottom.menu.clear()
            tabs.forEachIndexed { index, tab ->
                bottom.menu.add(Menu.NONE, index + 1, index, tab.title).isEnabled = tab.enabled
                refreshTabIcon(installed, index)
                applyBadge(bottom, index + 1, tab.badge)
            }
        } finally {
            applyingSelection = false
        }
        val visibility = if (tabs.isEmpty()) View.GONE else View.VISIBLE
        if (bottom.visibility != visibility) {
            bottom.visibility = visibility
            ViewCompat.requestApplyInsets(installed.content)
        }
        applySelected(installed)
    }

    private fun refreshTabIcon(installed: Installed, index: Int) {
        val tab = installed.model.tabs.getOrNull(index) ?: return
        val item = installed.bars.bottom.menu.findItem(index + 1) ?: return
        val bitmap = icons[iconKey(ICON_SLOT_TAB, index)]
        item.icon = if (tab.iconRev != 0L && bitmap != null) BitmapDrawable(installed.bars.context.resources, bitmap) else null
    }

    private fun applyBadge(bottom: BottomNavigationView, itemId: Int, badge: String?) {
        if (badge == null) {
            bottom.removeBadge(itemId)
            return
        }
        val drawable = bottom.getOrCreateBadge(itemId)
        val number = badge.toIntOrNull()
        drawable.clearNumber()
        drawable.clearText()
        when {
            badge.isEmpty() -> Unit
            number != null && number >= 0 -> drawable.number = number
            else -> drawable.text = badge
        }
        drawable.isVisible = true
    }

    /** Show the app-confirmed tab; the selection listener accepts it under [applyingSelection]. */
    private fun applySelected(installed: Installed) {
        val bottom = installed.bars.bottom
        val menu = bottom.menu
        val index = installed.model.selected
        applyingSelection = true
        try {
            if (index in 0 until menu.size()) {
                bottom.selectedItemId = index + 1
            } else {
                menu.setGroupCheckable(Menu.NONE, true, false)
                for (i in 0 until menu.size()) menu.getItem(i).isChecked = false
                menu.setGroupCheckable(Menu.NONE, true, true)
            }
        } finally {
            applyingSelection = false
        }
    }

    // --- reporting -----------------------------------------------------------

    private fun emitTab(index: Int, reselect: Boolean) {
        if (index < 0) return
        val detail = index.toLong() or (if (reselect) 1L shl CHROME_TAB_RESELECT_SHIFT else 0L)
        emit(FrustNativeListener.KIND_CHROME_TAB, detail)
    }

    private fun emit(kind: Int, detail: Long) {
        val installed = live ?: return
        nativeOnChromeEvent(installed.generation, kind, detail)
    }

    // --- helpers -------------------------------------------------------------

    private fun warnTheme(t: Throwable) {
        Log.w(
            TAG,
            "Frust: native app chrome refused — the app theme must derive from Theme.Material3.* " +
                "(or Theme.MaterialComponents.*): set android:theme on <application> and <activity>, " +
                "e.g. @style/Theme.Material3.DayNight.NoActionBar. Falling back to frust-drawn bars. ($t)",
        )
    }

    private fun dismissPopup() {
        val shown = popup ?: return
        popup = null
        try {
            shown.dismiss()
        } catch (t: Throwable) {
            // Its window may already be gone.
        }
    }

    private fun assertMainThread() {
        if (debuggable) check(Looper.myLooper() == Looper.getMainLooper()) { "FrustNativeChrome called off the main thread" }
    }

    private fun iconKey(slot: Int, index: Int): Long = (slot.toLong() shl 32) or (index.toLong() and 0xFFFFFFFFL)

    private fun dp(context: Context, value: Int): Int = (value * context.resources.displayMetrics.density).roundToInt()

    /** `bitmap` scaled to fit a `size`-px square, aspect kept. */
    private fun fitScaled(bitmap: Bitmap, size: Int): Bitmap {
        val scale = size.toFloat() / max(bitmap.width, bitmap.height)
        if (scale == 1f) return bitmap
        val w = max(1, (bitmap.width * scale).roundToInt())
        val h = max(1, (bitmap.height * scale).roundToInt())
        return Bitmap.createScaledBitmap(bitmap, w, h, true)
    }

    /** `bitmap` centre-cropped to a square and scaled to `size` px. */
    private fun squareScaled(bitmap: Bitmap, size: Int): Bitmap {
        val side = minOf(bitmap.width, bitmap.height)
        val square = Bitmap.createBitmap(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side)
        return Bitmap.createScaledBitmap(square, size, size, true)
    }
}
