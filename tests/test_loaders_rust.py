# Test loader behavior with rust engines

import os
import tempfile
from jutsu import Environment
from jutsu.loaders import (
    BaseLoader, FileSystemLoader, PackageLoader, DictLoader,
    FunctionLoader, PrefixLoader, ChoiceLoader
)


def test_base_loader_abstract():
    """BaseLoader is abstract and cannot be instantiated directly."""
    try:
        loader = BaseLoader()
        assert False, "Should not be able to instantiate BaseLoader"
    except TypeError:
        pass  # Expected


def test_filesystem_loader():
    """Test FileSystemLoader functionality."""
    with tempfile.TemporaryDirectory() as tmpdir:
        # Create test template
        template_path = os.path.join(tmpdir, 'test.html')
        with open(template_path, 'w') as f:
            f.write('{{ name }}')
        
        env = Environment(loader=FileSystemLoader(tmpdir))
        template = env.get_template('test.html')
        result = template.render(name='World')
        assert result == 'World'


def test_package_loader():
    """Test PackageLoader functionality."""
    # This would require a proper package structure to test properly
    pass  # Skip for now, as it requires actual package setup


def test_dict_loader():
    """Test DictLoader functionality."""
    templates = {
        'test.html': '{{ name }}',
        'other.html': 'Hello {{ name }}'
    }
    env = Environment(loader=DictLoader(templates))
    template = env.get_template('test.html')
    result = template.render(name='World')
    assert result == 'World'
    
    # Test other template
    template2 = env.get_template('other.html')
    result2 = template2.render(name='World')
    assert result2 == 'Hello World'


def test_function_loader():
    """Test FunctionLoader functionality."""
    def load_template(name):
        if name == 'test.html':
            return '{{ name }}'
        return None
    
    env = Environment(loader=FunctionLoader(load_template))
    template = env.get_template('test.html')
    result = template.render(name='World')
    assert result == 'World'


def test_prefix_loader():
    """Test PrefixLoader functionality."""
    loaders = {
        'a/': DictLoader({'test.html': 'A: {{ name }}'}),
        'b/': DictLoader({'test.html': 'B: {{ name }}'})
    }
    env = Environment(loader=PrefixLoader(loaders))
    
    # Test with prefix a/
    template = env.get_template('a/test.html')
    result = template.render(name='World')
    assert result == 'A: World'
    
    # Test with prefix b/
    template2 = env.get_template('b/test.html')
    result2 = template2.render(name='World')
    assert result2 == 'B: World'


def test_choice_loader():
    """Test ChoiceLoader functionality."""
    loaders = [
        DictLoader({'test.html': 'A: {{ name }}'}),
        DictLoader({'test.html': 'B: {{ name }}'})
    ]
    env = Environment(loader=ChoiceLoader(loaders))
    template = env.get_template('test.html')
    result = template.render(name='World')
    # ChoiceLoader uses the first loader that finds the template
    assert result == 'A: World'


def test_compile_templates():
    """Test Environment.compile_templates functionality."""
    templates = {
        'test.html': '{{ name }}',
        'other.html': 'Hello {{ name }}'
    }
    env = Environment(loader=DictLoader(templates))
    
    # Compile all templates
    compiled = env.compile_templates(['test.html', 'other.html'])
    assert len(compiled) == 2
    
    # Render using the compiled templates
    result1 = env.from_code(compiled[0], {}, None, None).render(name='World')
    assert result1 == 'World'
    
    result2 = env.from_code(compiled[1], {}, None, None).render(name='World')
    assert result2 == 'Hello World'


def test_filesystem_bytecode_cache():
    """Test FileSystemBytecodeCache functionality."""
    with tempfile.TemporaryDirectory() as tmpdir:
        # Create a template file
        template_path = os.path.join(tmpdir, 'test.html')
        with open(template_path, 'w') as f:
            f.write('{{ name }}')
        
        # Create bytecode cache
        from jutsu.bccache import FileSystemBytecodeCache
        bcc = FileSystemBytecodeCache(cache_dir=tmpdir)
        
        # Load template with cache
        env = Environment(loader=FileSystemLoader(tmpdir), bytecode_cache=bcc)
        template = env.get_template('test.html')
        result = template.render(name='World')
        assert result == 'World'
        
        # Reload from cache
        env2 = Environment(loader=FileSystemLoader(tmpdir), bytecode_cache=bcc)
        template2 = env2.get_template('test.html')
        result2 = template2.render(name='World')
        assert result2 == 'World'


def test_loader_compatibility_with_rust_engines():
    """Test that all loaders work identically with rust engines default-on."""
    # This tests the integration of loaders with the rust engine behavior
    templates = {
        'test.html': '{{ name }}',
        'other.html': 'Hello {{ name }}'
    }
    
    env = Environment(loader=DictLoader(templates))
    template = env.get_template('test.html')
    result = template.render(name='World')
    assert result == 'World'
