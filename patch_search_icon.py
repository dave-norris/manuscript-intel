with open('/Users/norris/Developer/pub-rocket-reader/src-tauri/src/pr_scraper.rs', 'r') as f:
    content = f.read()

old = '''fn type_into_search(session: &mut cdp::Session, category: &str) {
    // Use first word before & or , to avoid React input issues with special chars
    let search_term = category.split(|c| c == \'&\' || c == \',\')
        .next().unwrap_or(category).trim().to_string();
    let sj = serde_json::to_string(&search_term).unwrap();

    let js = format!(r#"
        const input = document.querySelector(
            \'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"]\'
        );
        if (input) {{
            input.focus();
            input.value = \'\';
            input.dispatchEvent(new Event(\'input\',{{bubbles:true}}));
            const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,\'value\').set;
            setter.call(input, {s});
            input.dispatchEvent(new Event(\'input\',{{bubbles:true}}));
            input.dispatchEvent(new Event(\'change\',{{bubbles:true}}));
        }}
        return \'\';
    "#, s = sj);
    let _ = session.eval(&js, 8);
}'''

new = '''fn type_into_search(session: &mut cdp::Session, category: &str) {
    // Use first word before & or , to avoid React input issues with special chars
    let search_term = category.split(|c| c == \'&\' || c == \',\')
        .next().unwrap_or(category).trim().to_string();
    let sj = serde_json::to_string(&search_term).unwrap();

    // Click the search icon first — PR hides the input until the icon is clicked
    let click_search_icon_js = r#"
        // Look for a search icon button near the radio buttons
        const btn = Array.from(document.querySelectorAll(\'button,span,a,div\'))
          .find(e => {
            const txt = e.textContent.trim();
            const cls = (e.className || \'\').toLowerCase();
            return txt === \'\' && (
              cls.includes(\'search\') || cls.includes(\'magnif\') ||
              e.querySelector(\'svg,img\') !== null
            ) && e.getBoundingClientRect().width < 60;
          });
        if (btn) {
          btn.scrollIntoView({block:\'center\'});
          const r = btn.getBoundingClientRect();
          return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        }
        // Fallback: look for any clickable element with a search-related aria-label
        const aria = document.querySelector(\'[aria-label*="search" i],[aria-label*="Search" i]\');
        if (aria) {
          const r = aria.getBoundingClientRect();
          return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        }
        return JSON.stringify(null);
    "#;

    if let Ok(s) = session.eval(click_search_icon_js, 8) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }

    let js = format!(r#"
        const input = document.querySelector(
            \'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"]\'
        );
        if (input) {{
            input.focus();
            input.value = \'\';
            input.dispatchEvent(new Event(\'input\',{{bubbles:true}}));
            const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,\'value\').set;
            setter.call(input, {s});
            input.dispatchEvent(new Event(\'input\',{{bubbles:true}}));
            input.dispatchEvent(new Event(\'change\',{{bubbles:true}}));
        }}
        return input ? \'ok\' : \'no input found\';
    "#, s = sj);
    if let Ok(result) = session.eval(&js, 8) {
        // result will tell us if the input was found
        let _ = result;
    }
}'''

if old in content:
    content = content.replace(old, new, 1)
    with open('/Users/norris/Developer/pub-rocket-reader/src-tauri/src/pr_scraper.rs', 'w') as f:
        f.write(content)
    print('REPLACED')
else:
    print('NOT FOUND - trying alternative approach')
    idx = content.find('fn type_into_search')
    print(repr(content[idx:idx+200]))
